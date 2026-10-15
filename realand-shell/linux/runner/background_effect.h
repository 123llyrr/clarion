// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef REALAND_SHELL_BACKGROUND_EFFECT_H_
#define REALAND_SHELL_BACKGROUND_EFFECT_H_

#include <gtk/gtk.h>
#include <wayland-client.h>

#include "background_effect_region.h"

struct ext_background_effect_manager_v1;
struct ext_background_effect_surface_v1;

// Opts a GTK surface into compositor blur. Discovery is asynchronous on GDK's
// Wayland queue; no roundtrip or separate event dispatch blocks GTK/Flutter.
class BackgroundEffect final {
 public:
  explicit BackgroundEffect(GtkWidget* widget);
  ~BackgroundEffect();
  BackgroundEffect(const BackgroundEffect&) = delete;
  BackgroundEffect& operator=(const BackgroundEffect&) = delete;
  void SetRegion(bool enabled, double x, double y, double width, double height,
                 double radius = 0);
  void SetRegions(const std::vector<realand_shell::GlassRegion>& regions);
  void Refresh();
  bool Available() const;
  bool Requested() const { return enabled_; }
  int AppliedWidth() const { return applied_width_; }
  int AppliedHeight() const { return applied_height_; }
  const std::vector<realand_shell::BlurRect>& AppliedRegions() const {
    return applied_regions_;
  }

 private:
  static void Global(void*, wl_registry*, uint32_t, const char*, uint32_t);
  static void GlobalRemoved(void*, wl_registry*, uint32_t);
  static void Capabilities(void*, ext_background_effect_manager_v1*, uint32_t);
  static void Realized(GtkWidget*, gpointer);
  static void Unrealized(GtkWidget*, gpointer);
  static void Mapped(GtkWidget*, gpointer);
  static void Unmapped(GtkWidget*, gpointer);
  void ReleaseSurface();
  void Apply();

  GtkWidget* widget_ = nullptr;
  wl_display* display_ = nullptr;  // Borrowed from GDK.
  wl_registry* registry_ = nullptr;
  wl_compositor* compositor_ = nullptr;
  ext_background_effect_manager_v1* manager_ = nullptr;
  ext_background_effect_surface_v1* effect_ = nullptr;
  wl_surface* surface_ = nullptr;  // Borrowed from GDK, never committed here.
  uint32_t manager_name_ = 0;
  uint32_t compositor_name_ = 0;
  bool blur_capability_ = false;
  bool enabled_ = false;
  bool applied_ = false;
  bool region_set_ = false;
  int applied_width_ = -1, applied_height_ = -1;
  std::vector<realand_shell::GlassRegion> regions_;
  std::vector<realand_shell::BlurRect> applied_regions_;
  gulong realize_handler_ = 0, unrealize_handler_ = 0;
  gulong map_handler_ = 0, unmap_handler_ = 0;
};
#endif
