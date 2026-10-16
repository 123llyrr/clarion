// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_layout.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/ui/shell_geometry.dart';

void main() {
  test(
    'macOS reserves both menu bar and Dock regardless of saved bar edge',
    () {
      for (final position in BarPosition.values) {
        final insets = ShellGeometry.desktopInsets(ShellStyle.macos, position);
        expect(insets.top, 22);
        expect(insets.bottom, const MacShellPreferences().dockSize + 16);
        final grid = DesktopGridSpec.fromSize(
          const Size(960, 600),
          barPosition: position,
          surfaceInsets: insets,
        );
        expect(grid.area, const Rect.fromLTWH(16, 38, 928, 482));
        final positions = DesktopLayout.resolve(
          List.generate(grid.capacity + 1, (i) => 'file-$i'),
          grid,
          const {},
        );
        for (final cell in positions.values) {
          final rect = grid.tileRect(cell);
          expect(rect.top, greaterThanOrEqualTo(22));
          expect(rect.bottom, lessThanOrEqualTo(600 - insets.bottom));
          expect(grid.cellAt(rect.center, page: cell.page), cell);
        }
        expect(positions.values.any((cell) => cell.page == 1), isTrue);
      }
    },
  );

  test('Material keeps exactly its original single bar reserved area', () {
    for (final position in BarPosition.values) {
      final oldGrid = DesktopGridSpec.fromSize(
        const Size(960, 600),
        barPosition: position,
      );
      final sharedGrid = DesktopGridSpec.fromSize(
        const Size(960, 600),
        barPosition: position,
        surfaceInsets: ShellGeometry.desktopInsets(
          ShellStyle.material,
          position,
        ),
      );
      expect(sharedGrid.area, oldGrid.area);
      expect(sharedGrid.capacity, oldGrid.capacity);
    }
  });

  test('surface dimensions and hit tests use logical output pixels', () {
    for (final scale in [1.0, 1.25, 1.5, 2.0]) {
      final physicalSize = Size(960 * scale, 600 * scale);
      final grid = DesktopGridSpec.fromSize(
        physicalSize / scale,
        barPosition: BarPosition.bottom,
        surfaceInsets: ShellGeometry.desktopInsets(
          ShellStyle.macos,
          BarPosition.bottom,
        ),
      );
      final cell = DesktopCell(grid.columns - 1, grid.rows - 1);
      expect(grid.cellAt(grid.tileRect(cell).center), cell);
      expect(grid.area.bottom, 520);
      expect(grid.cellAt(const Offset(30, 31)), isNull);
      expect(grid.cellAt(const Offset(30, 550)), isNull);
    }
    expect(ShellGeometry.barHeightFor(ShellStyle.material), 40);
    expect(ShellGeometry.barHeightFor(ShellStyle.macos), 22);
    expect(ShellGeometry.tooltipHeightFor(ShellStyle.material), 128);
    expect(ShellGeometry.tooltipHeightFor(ShellStyle.macos), 110);
    expect(
      ShellGeometry.dockSurfaceHeight,
      greaterThan(ShellGeometry.dockHeight),
    );
  });

  test('macOS Dock edges, sizes and hidden bars bound every grid hit', () {
    const size = Size(960, 600);
    for (final dockPosition in MacDockPosition.values) {
      for (final dockSize in [32.0, 48.0, 64.0]) {
        for (final hidden in [false, true]) {
          final preferences = MacShellPreferences(
            dockPosition: dockPosition,
            dockSize: dockSize,
            dockAutoHide: hidden,
            menuBarAutoHide: hidden,
          );
          final dock = hidden ? 0.0 : dockSize + 16;
          final insets = ShellGeometry.desktopInsets(
            ShellStyle.macos,
            BarPosition.bottom,
            macPreferences: preferences,
          );
          expect(insets.top, hidden ? 0 : 22);
          expect(insets.left, dockPosition == MacDockPosition.left ? dock : 0);
          expect(
            insets.right,
            dockPosition == MacDockPosition.right ? dock : 0,
          );
          expect(
            insets.bottom,
            dockPosition == MacDockPosition.bottom ? dock : 0,
          );
          final grid = DesktopGridSpec.fromSize(
            size,
            barPosition: BarPosition.bottom,
            surfaceInsets: insets,
          );
          final safeArea = Rect.fromLTRB(
            insets.left,
            insets.top,
            size.width - insets.right,
            size.height - insets.bottom,
          );
          final positions = DesktopLayout.resolve(
            List.generate(grid.capacity + 1, (index) => 'file-$index'),
            grid,
            const {},
          );
          for (final cell in positions.values) {
            final rect = grid.tileRect(cell);
            expect(safeArea.contains(rect.topLeft), isTrue);
            expect(safeArea.contains(rect.bottomRight), isTrue);
            expect(grid.cellAt(rect.center, page: cell.page), cell);
          }
          if (!hidden) {
            final dockPoint = switch (dockPosition) {
              MacDockPosition.bottom => Offset(size.width / 2, size.height - 1),
              MacDockPosition.left => Offset(1, size.height / 2),
              MacDockPosition.right => Offset(size.width - 1, size.height / 2),
            };
            expect(grid.cellAt(dockPoint), isNull);
          }
        }
      }
    }
  });

  test('hidden macOS preferences do not change Material reserved edges', () {
    for (final position in BarPosition.values) {
      expect(
        ShellGeometry.desktopInsets(
          ShellStyle.material,
          position,
          macPreferences: const MacShellPreferences(
            dockPosition: MacDockPosition.left,
            dockSize: 64,
            dockAutoHide: true,
            menuBarAutoHide: true,
          ),
        ),
        position == BarPosition.top
            ? const EdgeInsets.only(top: 40)
            : const EdgeInsets.only(bottom: 40),
      );
    }
  });

  test(
    'configured menu height protects desktop tiles at every output scale',
    () {
      for (final scale in [1.0, 1.25, 1.5, 2.0]) {
        for (final height in [22.0, 32.0, 48.0]) {
          final prefs = MacShellPreferences(menuBarHeight: height);
          final insets = ShellGeometry.desktopInsets(
            ShellStyle.macos,
            BarPosition.bottom,
            macPreferences: prefs,
          );
          expect(insets.top, height);
          expect(
            ShellGeometry.barHeightFor(ShellStyle.macos, macPreferences: prefs),
            height,
          );
          expect(
            ShellGeometry.tooltipHeightFor(
              ShellStyle.macos,
              macPreferences: prefs,
            ),
            height + 88,
          );
          expect(
            ShellGeometry.barHeightFor(
              ShellStyle.material,
              macPreferences: prefs,
            ),
            40,
          );
          final grid = DesktopGridSpec.fromSize(
            Size(960 * scale, 600 * scale) / scale,
            barPosition: BarPosition.bottom,
            surfaceInsets: insets,
          );
          final first = grid.tileRect(const DesktopCell(0, 0));
          expect(grid.area.top, height + 16);
          expect(first.top, greaterThanOrEqualTo(grid.area.top));
          expect(grid.cellAt(first.center), const DesktopCell(0, 0));
          expect(grid.cellAt(Offset(first.center.dx, height - 1)), isNull);
          expect(
            ShellGeometry.desktopInsets(
              ShellStyle.macos,
              BarPosition.top,
              macPreferences: prefs.copyWith(menuBarAutoHide: true),
            ).top,
            0,
          );
        }
      }
    },
  );

  test('small outputs never produce negative desktop geometry', () {
    final grid = DesktopGridSpec.fromSize(
      const Size(20, 64),
      barPosition: BarPosition.top,
      surfaceInsets: ShellGeometry.desktopInsets(
        ShellStyle.macos,
        BarPosition.top,
      ),
    );
    expect(grid.capacity, 0);
    expect(grid.area.width, 0);
    expect(grid.area.height, 0);
    expect(grid.cellAt(Offset.zero), isNull);
  });
}
