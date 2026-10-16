// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_DOCK_INPUT_REGION_H_
#define CLARION_SHELL_DOCK_INPUT_REGION_H_

#include <cairo.h>
#include <cmath>
#include <vector>

#include "shell_geometry.h"

namespace clarion_shell {
// Flutter supplies logical surface coordinates, including fractional transforms.
// Enclose each rectangle once at the native integer boundary, without dropping
// the left/top strip or extending its right/bottom by rounding width separately.
inline bool EncloseDockInputRect(double x, double y, double width, double height,
                                 InputRect* result) {
  for (const double value : {x, y, width, height}) {
    if (!std::isfinite(value) || std::abs(value) > 1000000) return false;
  }
  if (width < 0 || height < 0) return false;
  const int left = static_cast<int>(std::floor(x));
  const int top = static_cast<int>(std::floor(y));
  *result = {left, top,
             width == 0 ? 0 : static_cast<int>(std::ceil(x + width)) - left,
             height == 0 ? 0 : static_cast<int>(std::ceil(y + height)) - top};
  return true;
}

// The caller owns the returned Cairo region. Preserve the holes between raised
// icons; an enclosing bounding box would steal clicks from windows behind them.
inline cairo_region_t* BuildDockInputRegion(const std::vector<InputRect>& requested,
                                            int width, int height,
                                            bool legacy_corners = false) {
  cairo_region_t* region = cairo_region_create();
  for (const InputRect& source : requested) {
    const InputRect rect = DockInputRect(source, width, height);
    if (rect.width <= 0 || rect.height <= 0) continue;
    const cairo_rectangle_int_t input = {rect.x, rect.y, rect.width, rect.height};
    cairo_region_union_rectangle(region, &input);
    if (!legacy_corners) continue;
    const int corner = std::min(6, std::min(rect.width, rect.height) / 2);
    for (int edge = 0; edge < 2; ++edge) {
      for (int row = 0; row < 2; ++row) {
        const cairo_rectangle_int_t cut = {
            rect.x + (edge == 0 ? 0 : rect.width - corner),
            rect.y + (row == 0 ? 0 : rect.height - corner), corner, corner};
        cairo_region_subtract_rectangle(region, &cut);
      }
    }
  }
  return region;
}
}  // namespace clarion_shell

#endif  // CLARION_SHELL_DOCK_INPUT_REGION_H_
