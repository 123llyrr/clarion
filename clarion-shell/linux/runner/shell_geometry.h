// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_GEOMETRY_H_
#define CLARION_SHELL_GEOMETRY_H_

#include <algorithm>

namespace clarion_shell {
// Main bar default sizes and reservations match Dart's shell geometry. Dock
// drawing extends above its reservation to make room for icon magnification.
constexpr int kBarHeight = 40;
constexpr int kMacosBarHeight = 22;
constexpr int kMaxMacosBarHeight = 48;
constexpr int kDockHeight = 84;
constexpr int kDockSurfaceHeight = 120;
constexpr int kDefaultDockWidth = 720;
enum class DockPosition { Bottom, Left, Right };

struct DockPlacement {
  int width;
  int height;
  int x;
  int y;
};

inline DockPlacement PlaceDock(DockPosition position, int desired_width,
                               int desired_height, int output_width,
                               int output_height) {
  const bool bottom = position == DockPosition::Bottom;
  const int max_width = std::max(1, output_width - (bottom ? 48 : 0));
  const int max_height = std::max(1, output_height - (bottom ? 0 : 48));
  const int width = std::max(1, std::min(desired_width, max_width));
  const int height = std::max(1, std::min(desired_height, max_height));
  return {width, height,
          bottom ? std::max(0, (output_width - width) / 2)
                 : position == DockPosition::Left ? 0
                                                 : std::max(0, output_width - width),
          bottom ? std::max(0, output_height - height)
                 : std::max(0, (output_height - height) / 2)};
}
// Tooltips need drawable space below the strip, but reserve and receive input
// only in the strip itself. Keep this aligned with ShellGeometry.tooltipHeight.
constexpr int kTooltipHeight = 128;

constexpr int BarHeight(bool macos, int macos_height = kMacosBarHeight) {
  return macos ? (macos_height < kMacosBarHeight ? kMacosBarHeight
      : macos_height > kMaxMacosBarHeight ? kMaxMacosBarHeight : macos_height)
      : kBarHeight;
}

inline int BarInputHeight(bool macos, bool auto_hide, bool visible, int height,
                          int macos_height = kMacosBarHeight) {
  return std::max(0, std::min(macos && auto_hide && !visible ? 2
      : BarHeight(macos, macos_height), height));
}

constexpr int MainTooltipHeight(bool macos, int macos_height = kMacosBarHeight) {
  return BarHeight(macos, macos_height) + kTooltipHeight - kBarHeight;
}

inline int BarInputY(int height, bool bottom, bool expanded, bool preview,
                     bool macos) {
  return bottom && !macos && !preview && !expanded
      ? std::max(0, height - BarHeight(macos)) : 0;
}

// A single bottom anchor keeps this surface horizontally centered. Leave
// room at the output edges and clamp on every output geometry change.
inline int DockWidth(int desired, int output_width) {
  const int available = output_width > 0 ? std::max(1, output_width - 48)
                                         : kDefaultDockWidth;
  return std::max(1, std::min(desired, available));
}

struct InputRect {
  int x;
  int y;
  int width;
  int height;
};

// Rectangles supplied by Dart are in actual surface coordinates. The extra
// drawable space above the 84px reservation contains magnified icons; their
// reported hit regions must remain interactive.
inline InputRect DockInputRect(InputRect rect, int width, int surface_height) {
  const int input_height = std::max(0, surface_height);
  const int left = std::max(0, std::min(rect.x, width));
  const int top = std::max(0, std::min(rect.y, input_height));
  const int right = std::max(left, std::min(rect.x + rect.width, width));
  const int bottom = std::max(top, std::min(rect.y + rect.height, input_height));
  return {left, top, right - left, bottom - top};
}
}  // namespace clarion_shell

#endif  // CLARION_SHELL_GEOMETRY_H_
