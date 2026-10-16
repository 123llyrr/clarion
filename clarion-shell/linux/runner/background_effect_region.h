// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_BACKGROUND_EFFECT_REGION_H_
#define CLARION_SHELL_BACKGROUND_EFFECT_REGION_H_

#include <cairo.h>

#include <algorithm>
#include <cmath>
#include <vector>

namespace clarion_shell {
struct GlassRegion {
  double x = 0, y = 0, width = 0, height = 0, radius = 0;
  bool operator==(const GlassRegion& other) const {
    return x == other.x && y == other.y && width == other.width &&
        height == other.height && radius == other.radius;
  }
};

struct BlurRect {
  int x, y, width, height;
  bool operator==(const BlurRect& other) const {
    return x == other.x && y == other.y && width == other.width &&
        height == other.height;
  }
};

// Validate before integer conversion. A zero dimension is a valid clear;
// oversized radii clamp to the geometric maximum, never wrap integer bounds.
inline bool NormalizeGlassRegion(const GlassRegion& input, GlassRegion* result) {
  for (const double value : {input.x, input.y, input.width, input.height, input.radius}) {
    if (!std::isfinite(value) || std::abs(value) > 1000000) return false;
  }
  if (input.width < 0 || input.height < 0 || input.radius < 0) return false;
  *result = input;
  result->radius = std::min(input.radius, std::min(input.width, input.height) / 2);
  return true;
}

inline bool NormalizeGlassRegions(const std::vector<GlassRegion>& input,
                                  std::vector<GlassRegion>* result) {
  result->clear();
  if (input.size() > 64) return false;
  for (const auto& source : input) {
    GlassRegion region;
    if (!NormalizeGlassRegion(source, &region)) {
      result->clear();
      return false;
    }
    if (region.width > 0 && region.height > 0) result->push_back(region);
  }
  return true;
}

// The caller owns the returned Cairo region. Wayland regions use integer
// logical coordinates: enclose outer bounds and conservatively inset rounded
// scanlines. Compute the curve from the original rectangle BEFORE clipping;
// otherwise a partially offscreen panel acquires a false rounded screen edge.
inline cairo_region_t* BuildBackgroundEffectRegion(
    const std::vector<GlassRegion>& requested, int width, int height) {
  cairo_region_t* result = cairo_region_create();
  std::vector<GlassRegion> regions;
  if (width <= 0 || height <= 0 || !NormalizeGlassRegions(requested, &regions)) return result;
  for (const auto& rect : regions) {
    const int original_left = static_cast<int>(std::floor(rect.x));
    const int original_right = static_cast<int>(std::ceil(rect.x + rect.width));
    const int top = std::max(0, static_cast<int>(std::floor(rect.y)));
    const int bottom = std::min(height, static_cast<int>(std::ceil(rect.y + rect.height)));
    const int left = std::max(0, original_left);
    const int right = std::min(width, original_right);
    if (left >= right || top >= bottom) continue;
    if (rect.radius == 0) {
      const cairo_rectangle_int_t band = {left, top, right - left, bottom - top};
      cairo_region_union_rectangle(result, &band);
      continue;
    }
    int start = top, previous_left = left, previous_right = left;
    for (int row = top; row < bottom; ++row) {
      const double center_y = row + .5;
      const double distance = std::max(0.0, std::max(rect.y + rect.radius - center_y,
          center_y - (rect.y + rect.height - rect.radius)));
      const int inset = static_cast<int>(std::ceil(rect.radius -
          std::sqrt(std::max(0.0, rect.radius * rect.radius - distance * distance))));
      const int row_left = std::max(0, original_left + inset);
      const int row_right = std::min(width, original_right - inset);
      if (row > top && (row_left != previous_left || row_right != previous_right)) {
        if (previous_left < previous_right) {
          const cairo_rectangle_int_t band = {
              previous_left, start, previous_right - previous_left, row - start};
          cairo_region_union_rectangle(result, &band);
        }
        start = row;
      }
      previous_left = row_left;
      previous_right = row_right;
    }
    if (previous_left < previous_right) {
      const cairo_rectangle_int_t band = {
          previous_left, start, previous_right - previous_left, bottom - start};
      cairo_region_union_rectangle(result, &band);
    }
  }
  return result;
}

inline std::vector<BlurRect> BackgroundEffectRectangles(cairo_region_t* region) {
  std::vector<BlurRect> result;
  const int count = cairo_region_num_rectangles(region);
  result.reserve(count);
  for (int index = 0; index < count; ++index) {
    cairo_rectangle_int_t rect;
    cairo_region_get_rectangle(region, index, &rect);
    result.push_back({rect.x, rect.y, rect.width, rect.height});
  }
  return result;
}
}  // namespace clarion_shell

#endif  // CLARION_SHELL_BACKGROUND_EFFECT_REGION_H_
