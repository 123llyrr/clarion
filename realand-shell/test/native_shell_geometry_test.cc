// SPDX-License-Identifier: GPL-3.0-or-later
// Run with: c++ -std=c++14 -Wall -Werror test/native_shell_geometry_test.cc
//             -o /tmp/realand-shell-geometry-test && /tmp/realand-shell-geometry-test
#include "../linux/runner/shell_geometry.h"

#include <cassert>
#include <iostream>

int main() {
  using namespace realand_shell;
  static_assert(BarHeight(false) == 40, "Material reservation must stay 40px");
  static_assert(BarHeight(true) == 22, "macOS menu reservation must be 22px");
  static_assert(kDockHeight == 84, "Dock reservation must be 84px");
  static_assert(kDockSurfaceHeight == 120, "Magnification plus bounce needs 120px surface");
  static_assert(MainTooltipHeight(false) == 128, "Material tooltips keep 128px");
  static_assert(MainTooltipHeight(true) == 110, "22px menu plus 88px tooltip space");
  static_assert(BarHeight(true, 32) == 32, "Menu height controls its reservation");
  static_assert(BarHeight(true, 48) == 48, "Largest supported menu height");
  static_assert(BarHeight(true, 0) == 22, "Invalid undersized height is clamped");
  static_assert(BarHeight(true, 100) == 48, "Invalid oversized height is clamped");
  static_assert(BarHeight(false, 48) == 40, "Material is independent of Mac height");
  static_assert(MainTooltipHeight(true, 48) == 136, "Tooltip space follows height");

  // Material bottom tooltips grow upward; macOS always keeps the menu at top.
  assert(BarInputY(128, true, false, false, false) == 88);
  assert(BarInputY(110, true, false, false, true) == 0);
  assert(BarInputY(1080, true, true, false, false) == 0);
  assert(BarInputY(800, true, false, true, false) == 0);
  assert(BarInputY(20, true, false, false, false) == 0);

  // Centered Dock surfaces can grow with application count without spanning
  // the whole output; small or removed outputs remain valid.
  assert(DockWidth(720, 1920) == 720);
  assert(DockWidth(2000, 1920) == 1872);
  assert(DockWidth(720, 640) == 592);
  assert(DockWidth(720, 32) == 1);
  assert(DockWidth(720, 0) == 720);

  // Side layouts keep icons upright, center their long axis, and use the same
  // output-local origin as native surfaceOrigin/Genie target registration.
  const auto bottom = PlaceDock(DockPosition::Bottom, 720, 120, 1920, 1080);
  assert(bottom.width == 720 && bottom.height == 120 && bottom.x == 600 && bottom.y == 960);
  const auto left_dock = PlaceDock(DockPosition::Left, 178, 720, 1920, 1080);
  assert(left_dock.width == 178 && left_dock.height == 720 && left_dock.x == 0 && left_dock.y == 180);
  const auto right_dock = PlaceDock(DockPosition::Right, 178, 2000, 1920, 1080);
  assert(right_dock.width == 178 && right_dock.height == 1032 && right_dock.x == 1742 && right_dock.y == 24);
  const auto tiny_dock = PlaceDock(DockPosition::Left, 120, 720, 32, 32);
  assert(tiny_dock.width == 32 && tiny_dock.height == 1 && tiny_dock.x == 0 && tiny_dock.y == 15);
  assert(BarInputHeight(true, true, false, 22) == 2);
  assert(BarInputHeight(true, true, true, 22) == 22);
  assert(BarInputHeight(false, true, false, 40) == 40);
  assert(BarInputHeight(true, true, false, 1) == 1);
  for (int menu_height : {22, 32, 48}) {
    assert(BarInputHeight(true, false, true, 200, menu_height) == menu_height);
    assert(BarInputHeight(true, true, true, menu_height, menu_height) == menu_height);
    assert(BarInputHeight(true, true, false, menu_height, menu_height) == 2);
    assert(BarInputHeight(true, false, true, 12, menu_height) == 12);
    assert(BarInputHeight(false, false, true, 200, menu_height) == 40);
  }

  InputRect normal = DockInputRect({0, 20, 720, 100}, 720, 120);
  assert(normal.x == 0 && normal.y == 20 && normal.width == 720 && normal.height == 100);
  // The magnified icon input above the 84px reservation remains interactive;
  // the 20px label area above it stays outside this rectangle.
  assert(normal.y < kDockSurfaceHeight - kDockHeight);
  assert(normal.y + normal.height == kDockSurfaceHeight);
  InputRect tooltip = DockInputRect({0, 20, 720, 100}, 720, 128);
  assert(tooltip.x == 0 && tooltip.y == 20 && tooltip.width == 720 && tooltip.height == 100);

  InputRect clipped = DockInputRect({-20, -10, 2000, 2000}, 592, 120);
  assert(clipped.x == 0 && clipped.y == 0 && clipped.width == 592 && clipped.height == 120);
  InputRect outside = DockInputRect({800, 130, 20, 20}, 720, 120);
  assert(outside.width == 0 && outside.height == 0);
  InputRect empty = DockInputRect({10, 10, 0, 0}, 720, 120);
  assert(empty.width == 0 && empty.height == 0);
  InputRect tiny = DockInputRect({0, 20, 720, 100}, 720, 1);
  assert(tiny.y == 1 && tiny.height == 0);
  std::cout << "Native shell geometry checks passed\n";
}
