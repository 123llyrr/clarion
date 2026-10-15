// SPDX-License-Identifier: GPL-3.0-or-later
// Pure production-region QA: no GTK display, compositor or Flutter required.
#include <cassert>
#include <iostream>
#include <limits>
#include <memory>

#include "background_effect_region.h"

using namespace realand_shell;
using Region = std::unique_ptr<cairo_region_t, decltype(&cairo_region_destroy)>;

static Region Build(const std::vector<GlassRegion>& regions, int width = 800,
                    int height = 600) {
  return Region(BuildBackgroundEffectRegion(regions, width, height), cairo_region_destroy);
}

static void CheckUnionKeepsDesktopGap() {
  auto region = Build({{0, 0, 800, 22, 0}, {468, 34, 320, 330, 18}});
  assert(cairo_region_contains_point(region.get(), 2, 21));
  assert(cairo_region_contains_point(region.get(), 500, 80));
  assert(!cairo_region_contains_point(region.get(), 400, 80));
  assert(!cairo_region_contains_point(region.get(), 600, 28));
  assert(!cairo_region_contains_point(region.get(), 468, 34));
  assert(!cairo_region_contains_point(region.get(), 787, 363));
  assert(!cairo_region_contains_point(region.get(), 790, 80));
  // Overlapping corners are a union, not corner cuts from a bounding box.
  auto overlap = Build({{0, 0, 800, 22, 0}, {0, 12, 320, 330, 18}});
  assert(cairo_region_contains_point(overlap.get(), 0, 12));
  assert(!cairo_region_contains_point(overlap.get(), 0, 341));
}

static void CheckClippingUsesOriginalCurve() {
  for (const GlassRegion requested : {
      GlassRegion{-20, -15, 100, 90, 24}, {-20, 7, 100, 90, 24},
      {13, -15, 100, 90, 24}, {13.25, 7.75, 100.5, 90.5, 24.25},
      {-20.25, -15.75, 100.5, 90.5, 24.25}}) {
    auto actual = Build({requested}, 64, 55);
    auto moved = requested;
    moved.x += 64;
    moved.y += 64;
    auto reference = Build({moved}, 240, 240);
    cairo_region_translate(reference.get(), -64, -64);
    const cairo_rectangle_int_t surface = {0, 0, 64, 55};
    cairo_region_intersect_rectangle(reference.get(), &surface);
    assert(cairo_region_equal(actual.get(), reference.get()));
    for (const auto& rect : BackgroundEffectRectangles(actual.get())) {
      assert(rect.x >= 0 && rect.y >= 0 && rect.x + rect.width <= 64 &&
          rect.y + rect.height <= 55 && rect.width > 0 && rect.height > 0);
    }
  }
}

static void CheckFractionalEndpointsAndCanonicalCache() {
  auto fractional = Build({{10.25, 20.75, 30.5, 40.5, 0}});
  const auto rectangles = BackgroundEffectRectangles(fractional.get());
  assert(rectangles.size() == 1);
  assert((rectangles[0] == BlurRect{10, 20, 31, 42}));
  auto jitter = Build({{10.30, 20.70, 30.5, 40.5, 0}});
  assert(rectangles == BackgroundEffectRectangles(jitter.get()));
  auto union_a = Build({{0, 0, 100, 40, 0}, {50, 0, 100, 40, 0}});
  auto union_b = Build({{0, 0, 150, 40, 0}});
  assert(BackgroundEffectRectangles(union_a.get()) == BackgroundEffectRectangles(union_b.get()));
  auto separated = Build({{0, 0, 30, 40, 0}, {120, 0, 30, 40, 0}});
  assert(BackgroundEffectRectangles(separated.get()) != BackgroundEffectRectangles(union_b.get()));
  auto rounded = Build({{10, 10, 320, 500, 18}});
  assert(cairo_region_num_rectangles(rounded.get()) < 40);
  auto fullscreen = Build({{0, 0, 800, 600, 0}});
  assert(cairo_region_num_rectangles(fullscreen.get()) == 1);
}

static void CheckValidationAndEmptyRegions() {
  const double nan = std::numeric_limits<double>::quiet_NaN();
  const double inf = std::numeric_limits<double>::infinity();
  for (const GlassRegion invalid : {
      GlassRegion{nan, 0, 10, 10, 0}, {0, inf, 10, 10, 0},
      {0, 0, nan, 10, 0}, {0, 0, 10, inf, 0}, {0, 0, 10, 10, nan},
      {0, 0, -1, 10, 0}, {0, 0, 10, -1, 0}, {0, 0, 10, 10, -1},
      {1000001, 0, 10, 10, 0}, {0, 0, 1000001, 10, 0}, {0, 0, 10, 10, 1000001}}) {
    GlassRegion normalized;
    assert(!NormalizeGlassRegion(invalid, &normalized));
    // Fail the whole internal batch, never apply a partial malformed request.
    auto region = Build({{0, 0, 20, 20, 0}, invalid});
    assert(cairo_region_is_empty(region.get()));
  }
  GlassRegion rounded;
  assert(NormalizeGlassRegion({10, 20, 100, 40, 1000000}, &rounded));
  assert(rounded.radius == 20);
  auto clamped = Build({rounded});
  auto expected = Build({{10, 20, 100, 40, 20}});
  assert(cairo_region_equal(clamped.get(), expected.get()));
  for (const auto& request : {std::vector<GlassRegion>{},
      std::vector<GlassRegion>{{0, 0, 0, 40, 0}},
      std::vector<GlassRegion>{{0, 0, 40, 0, 0}},
      std::vector<GlassRegion>{{900, 700, 100, 100, 12}},
      std::vector<GlassRegion>{{-200, -200, 100, 100, 12}},
      std::vector<GlassRegion>(65, GlassRegion{0, 0, 20, 20, 0})}) {
    auto region = Build(request);
    assert(cairo_region_is_empty(region.get()));
  }
  assert(cairo_region_is_empty(Build({{0, 0, 10, 10, 0}}, 0, 600).get()));
  assert(cairo_region_is_empty(Build({{0, 0, 10, 10, 0}}, 800, -1).get()));
}

int main() {
  CheckUnionKeepsDesktopGap();
  CheckClippingUsesOriginalCurve();
  CheckFractionalEndpointsAndCanonicalCache();
  CheckValidationAndEmptyRegions();
  std::cout << "background_effect_region: 4 groups passed (union, clipping, cache geometry, validation)\n";
}
