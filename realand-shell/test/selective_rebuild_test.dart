// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/main.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/preferences.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/controller_selector.dart';
import 'package:realand_shell/ui/tray_icon.dart';

Finder _key(String key) => find.byKey(ValueKey(key));

class _Counter extends ChangeNotifier {
  _Counter(this.value);

  int value;
  final listeners = <VoidCallback>{};

  @override
  void addListener(VoidCallback listener) {
    super.addListener(listener);
    listeners.add(listener);
  }

  @override
  void removeListener(VoidCallback listener) {
    super.removeListener(listener);
    listeners.remove(listener);
  }

  void publish([int? next]) {
    if (next != null) value = next;
    notifyListeners();
  }
}

class _Controller extends ShellController {
  _Controller()
    : super(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      );

  int workspaceReads = 0;
  DateTime clock = DateTime(2026, 10, 8, 12, 34);

  @override
  DateTime get now => clock;

  @override
  List<ShellWorkspace> get workspaces {
    workspaceReads++;
    return super.workspaces;
  }

  void setClock(DateTime value) {
    clock = value;
    notifyListeners();
  }
}

Future<_Controller> _mount(WidgetTester tester) async {
  tester.view.physicalSize = const Size(1280, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final controller = _Controller()
    ..setThemeMode(ThemeMode.dark)
    ..setReduceMotion(true)
    ..selectPanel(ShellPanel.quickSettings);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    ShellApplication(
      controller: controller,
      preferences: const ShellPreferences(themeMode: ThemeMode.dark),
      preview: true,
      persist: false,
    ),
  );
  await tester.pumpAndSettle();
  return controller;
}

void main() {
  testWidgets(
    'selector filters notifications and replaces listener ownership',
    (tester) async {
      final first = _Counter(3);
      final second = _Counter(7);
      addTearDown(first.dispose);
      addTearDown(second.dispose);
      var builds = 0;
      Widget selected(_Counter counter, {int factor = 1}) => Directionality(
        textDirection: TextDirection.ltr,
        child: ControllerSelector<(int, String)>(
          key: const ValueKey('selection'),
          controller: counter,
          select: () => (counter.value * factor, 'units'),
          builder: (context, value) {
            builds++;
            return Text('${value.$1} ${value.$2}');
          },
        ),
      );

      await tester.pumpWidget(selected(first));
      expect(first.listeners.length, 1);
      expect(builds, 1);
      for (var frame = 0; frame < 30; frame++) {
        first.publish();
        await tester.pump(const Duration(milliseconds: 16));
      }
      expect(builds, 1, reason: 'equal records do not schedule a rebuild');
      first.publish(4);
      await tester.pump();
      expect(builds, 2);
      expect(find.text('4 units'), findsOneWidget);

      await tester.pumpWidget(selected(second));
      expect(first.listeners, isEmpty);
      expect(second.listeners.length, 1);
      expect(find.text('7 units'), findsOneWidget);
      final replacedBuilds = builds;
      first.publish(9);
      await tester.pump();
      expect(builds, replacedBuilds);
      second.publish(8);
      await tester.pump();
      expect(find.text('8 units'), findsOneWidget);

      await tester.pumpWidget(selected(second, factor: 2));
      expect(second.listeners.length, 1);
      expect(find.text('16 units'), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
      expect(second.listeners, isEmpty);
      second.publish(10);
      await tester.pump();
      expect(tester.takeException(), isNull);
    },
  );

  for (final key in ['volume', 'brightness']) {
    testWidgets('30 $key drag frames do not rebuild or derive the system bar', (
      tester,
    ) async {
      final controller = await _mount(tester);
      final items = <String, Widget>{};
      final buttons = <String, Widget>{};
      for (final name in [
        'bar-launcher',
        'bar-screenshot',
        'bar-notifications',
        'bar-quick-settings',
      ]) {
        items[name] = tester.widget(_key(name));
        buttons[name] = tester.widget(
          find.descendant(of: _key(name), matching: find.byType(IconButton)),
        );
      }
      var barBuilds = 0;
      final previousHook = debugOnRebuildDirtyWidget;
      debugOnRebuildDirtyWidget = (element, builtOnce) {
        previousHook?.call(element, builtOnce);
        if (element.widget.key == const ValueKey('system-bar-selector')) {
          barBuilds++;
        }
      };
      addTearDown(() => debugOnRebuildDirtyWidget = previousHook);
      controller.workspaceReads = 0;
      final slider = _key('$key-slider');
      final center = tester.getCenter(slider);
      final gesture = await tester.startGesture(center);
      await tester.pump(const Duration(milliseconds: 16));
      var previous = tester.widget<Slider>(slider).value;
      for (var frame = 0; frame < 30; frame++) {
        final offset = frame < 15 ? -70 + frame * 10 : 70 - (frame - 14) * 10;
        await gesture.moveTo(center + Offset(offset.toDouble(), 0));
        await tester.pump(const Duration(milliseconds: 16));
        final value = tester.widget<Slider>(slider).value;
        expect((value - previous).abs(), greaterThan(.001));
        expect(
          value,
          key == 'volume' ? controller.volume : controller.brightness,
        );
        previous = value;
        for (final name in items.keys) {
          expect(tester.widget(_key(name)), same(items[name]));
          expect(
            tester.widget(
              find.descendant(
                of: _key(name),
                matching: find.byType(IconButton),
              ),
            ),
            same(buttons[name]),
          );
        }
      }
      await gesture.up();
      await tester.pump();
      expect(barBuilds, 0);
      expect(controller.workspaceReads, 0);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets('real bar dependencies and inherited appearance still update', (
    tester,
  ) async {
    final controller = await _mount(tester);
    final launcher = tester.element(_key('bar-launcher'));
    final oldMuteWidget = tester.widget(_key('bar-volume'));
    await controller.toggleMute();
    await tester.pump();
    expect(tester.widget(_key('bar-volume')), isNot(same(oldMuteWidget)));
    expect(tester.widget<ShellBarTooltip>(_key('bar-volume')).message, '音量已静音');
    await controller.switchWorkspace(2);
    await tester.pump();
    expect(tester.widget<ChoiceChip>(_key('workspace-2')).selected, isTrue);
    expect(tester.widget<ChoiceChip>(_key('workspace-1')).selected, isFalse);

    Finder notificationBadge() => find.descendant(
      of: _key('bar-notifications'),
      matching: find.byType(Badge),
    );
    expect(tester.widget<Badge>(notificationBadge()).isLabelVisible, isTrue);
    await controller.dismissNotification(controller.notifications.single.id);
    await tester.pump();
    expect(tester.widget<Badge>(notificationBadge()).isLabelVisible, isFalse);
    expect(
      tester.widget<Badge>(notificationBadge()).label,
      isA<Text>().having((text) => text.data, 'count', '0'),
    );
    controller.setDoNotDisturb(true);
    await tester.pump();
    expect(
      find.descendant(
        of: _key('bar-notifications'),
        matching: find.byIcon(Icons.notifications_off_outlined),
      ),
      findsOneWidget,
    );

    final unchangedClockWidget = tester.widget(_key('bar-launcher'));
    controller.setClock(DateTime(2026, 10, 8, 12, 34, 59));
    await tester.pump();
    expect(tester.widget(_key('bar-launcher')), same(unchangedClockWidget));
    controller.setClock(DateTime(2026, 10, 8, 12, 35));
    await tester.pump();
    expect(find.text('12:35'), findsOneWidget);
    expect(
      tester.widget(_key('bar-launcher')),
      isNot(same(unchangedClockWidget)),
    );

    Material barMaterial() => tester.widget<Material>(
      find
          .ancestor(of: _key('bar-launcher'), matching: find.byType(Material))
          .first,
    );
    final darkColor = barMaterial().color;
    controller.setThemeMode(ThemeMode.light);
    await tester.pumpAndSettle();
    expect(barMaterial().color, isNot(darkColor));
    expect(
      barMaterial().color,
      Theme.of(
        tester.element(_key('bar-launcher')),
      ).colorScheme.surfaceContainer,
    );
    controller.setBarPosition(BarPosition.bottom);
    await tester.pumpAndSettle();
    expect(tester.getRect(_key('shell-system-bar')).bottom, 900);
    expect(tester.element(_key('bar-launcher')), same(launcher));
    controller.closePanel();
    await tester.pump();
    expect(
      tester
          .widget<IconButton>(
            find.descendant(
              of: _key('bar-quick-settings'),
              matching: find.byType(IconButton),
            ),
          )
          .isSelected,
      isFalse,
    );
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
