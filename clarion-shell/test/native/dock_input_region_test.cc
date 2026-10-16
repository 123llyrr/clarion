// SPDX-License-Identifier: GPL-3.0-or-later
// Pure Cairo regression: no display, compositor, or Flutter engine required.
#include <cassert>
#include <limits>
#include <memory>

#include "dock_input_region.h"

using namespace clarion_shell;
using Region = std::unique_ptr<cairo_region_t, decltype(&cairo_region_destroy)>;

int main() {
  // The high band alongside a raised icon remains transparent to native input.
  // Exercise the actual production Cairo union for bottom, left, and right.
  for (int orientation = 0; orientation < 3; ++orientation) {
    const auto transform = [orientation](InputRect rect) {
      if (orientation == 0) return rect;
      if (orientation == 1) return InputRect{120 - rect.y - rect.height,
          rect.x, rect.height, rect.width};
      return InputRect{rect.y, rect.x, rect.height, rect.width};
    };
    const int width = orientation == 0 ? 400 : 120;
    const int height = orientation == 0 ? 120 : 400;
    Region region(BuildDockInputRegion({transform({10, 60, 300, 60}),
        transform({60, 10, 50, 80})}, width, height), cairo_region_destroy);
    const auto contains = [&](int x, int y) {
      const auto point = transform({x, y, 1, 1});
      return cairo_region_contains_point(region.get(), point.x, point.y);
    };
    assert(contains(80, 20));  // Raised icon is still clickable.
    assert(contains(20, 80));  // Resting Dock is still clickable.
    assert(!contains(20, 20));  // Gap inside the bounding box passes through.
    assert(!contains(350, 80));
  }

  Region sensor(BuildDockInputRegion({{10, 118, 300, 2}}, 400, 120),
                cairo_region_destroy);
  assert(cairo_region_contains_point(sensor.get(), 10, 118));
  assert(cairo_region_contains_point(sensor.get(), 309, 119));
  assert(!cairo_region_contains_point(sensor.get(), 10, 117));

  Region clipped(BuildDockInputRegion({{-20, -20, 30, 30}, {390, 110, 50, 50},
      {500, 500, 20, 20}, {20, 20, 0, 10}}, 400, 120), cairo_region_destroy);
  assert(cairo_region_contains_point(clipped.get(), 0, 0));
  assert(cairo_region_contains_point(clipped.get(), 399, 119));
  assert(!cairo_region_contains_point(clipped.get(), 400, 119));
  assert(!cairo_region_contains_point(clipped.get(), 20, 20));
  Region empty(BuildDockInputRegion({}, 400, 120), cairo_region_destroy);
  assert(cairo_region_is_empty(empty.get()));

  InputRect rect = {};
  assert(EncloseDockInputRect(10.25, 20.75, 30.5, 40.5, &rect));
  assert(rect.x == 10 && rect.y == 20 && rect.width == 31 && rect.height == 42);
  assert(EncloseDockInputRect(10.25, 20.75, 0, 0, &rect));
  assert(rect.width == 0 && rect.height == 0);
  assert(!EncloseDockInputRect(0, 0, -.1, 10, &rect));
  assert(!EncloseDockInputRect(std::numeric_limits<double>::infinity(), 0,
      10, 10, &rect));
  assert(!EncloseDockInputRect(0, 0, std::numeric_limits<double>::quiet_NaN(),
      10, &rect));
  assert(!EncloseDockInputRect(1000001, 0, 10, 10, &rect));

  Region legacy(BuildDockInputRegion({{10, 20, 100, 60}}, 400, 120, true),
                cairo_region_destroy);
  assert(!cairo_region_contains_point(legacy.get(), 10, 20));
  assert(cairo_region_contains_point(legacy.get(), 30, 40));
}
