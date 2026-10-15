// SPDX-License-Identifier: GPL-3.0-or-later
#include "background_effect.h"

#include <gdk/gdkwayland.h>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <utility>

#include "ext-background-effect-v1-client-protocol.h"

BackgroundEffect::BackgroundEffect(GtkWidget* widget) : widget_(widget) {
  g_object_add_weak_pointer(G_OBJECT(widget_), reinterpret_cast<gpointer*>(&widget_));
  realize_handler_ = g_signal_connect(widget_, "realize", G_CALLBACK(Realized), this);
  unrealize_handler_ = g_signal_connect(widget_, "unrealize", G_CALLBACK(Unrealized), this);
  map_handler_ = g_signal_connect(widget_, "map", G_CALLBACK(Mapped), this);
  unmap_handler_ = g_signal_connect(widget_, "unmap", G_CALLBACK(Unmapped), this);
  GdkDisplay* display = gtk_widget_get_display(widget_);
  if (!GDK_IS_WAYLAND_DISPLAY(display)) return;
  display_ = gdk_wayland_display_get_wl_display(display);
  registry_ = wl_display_get_registry(display_);
  static const wl_registry_listener listener = {Global, GlobalRemoved};
  wl_registry_add_listener(registry_, &listener, this);
  wl_display_flush(display_);
}

BackgroundEffect::~BackgroundEffect() {
  ReleaseSurface();
  if (manager_ != nullptr) ext_background_effect_manager_v1_destroy(manager_);
  if (compositor_ != nullptr) wl_compositor_destroy(compositor_);
  if (registry_ != nullptr) wl_registry_destroy(registry_);
  if (widget_ != nullptr) {
    g_signal_handler_disconnect(widget_, realize_handler_);
    g_signal_handler_disconnect(widget_, unrealize_handler_);
    g_signal_handler_disconnect(widget_, map_handler_);
    g_signal_handler_disconnect(widget_, unmap_handler_);
    g_object_remove_weak_pointer(G_OBJECT(widget_), reinterpret_cast<gpointer*>(&widget_));
  }
}

bool BackgroundEffect::Available() const {
  return manager_ != nullptr && compositor_ != nullptr && blur_capability_;
}

void BackgroundEffect::SetRegion(bool enabled, double x, double y,
                                 double width, double height, double radius) {
  SetRegions(enabled ? std::vector<realand_shell::GlassRegion>{{x, y, width, height, radius}}
                     : std::vector<realand_shell::GlassRegion>{});
}

void BackgroundEffect::SetRegions(const std::vector<realand_shell::GlassRegion>& regions) {
  std::vector<realand_shell::GlassRegion> normalized;
  // Invalid internal requests fail closed. MethodChannel validation rejects
  // malformed arguments before calling this function or changing panel state.
  realand_shell::NormalizeGlassRegions(regions, &normalized);
  if (regions_ == normalized) { Refresh(); return; }
  regions_ = std::move(normalized);
  enabled_ = !regions_.empty();
  applied_ = false;
  Refresh();
}

void BackgroundEffect::Refresh() {
  // Hiding a layer window can destroy its Wayland surface without unrealizing
  // the widget. Never inspect or retain that borrowed proxy while unmapped.
  if (widget_ == nullptr || !gtk_widget_get_mapped(widget_)) {
    ReleaseSurface();
    return;
  }
  if (!gtk_widget_get_realized(widget_) || !Available()) return;
  GdkWindow* window = gtk_widget_get_window(widget_);
  if (window == nullptr || !GDK_IS_WAYLAND_WINDOW(window)) return;
  wl_surface* surface = gdk_wayland_window_get_wl_surface(window);
  if (surface != surface_) { ReleaseSurface(); surface_ = surface; }
  if (surface_ == nullptr) return;
  if (applied_width_ != gtk_widget_get_allocated_width(widget_) ||
      applied_height_ != gtk_widget_get_allocated_height(widget_)) applied_ = false;
  if (effect_ == nullptr && enabled_) {
    effect_ = ext_background_effect_manager_v1_get_background_effect(manager_, surface_);
  }
  Apply();
}

void BackgroundEffect::Apply() {
  if (effect_ == nullptr || applied_ || widget_ == nullptr) return;
  const int width = gtk_widget_get_allocated_width(widget_);
  const int height = gtk_widget_get_allocated_height(widget_);
  cairo_region_t* exact = realand_shell::BuildBackgroundEffectRegion(regions_, width, height);
  auto rectangles = realand_shell::BackgroundEffectRectangles(exact);
  cairo_region_destroy(exact);
  applied_ = true;
  applied_width_ = width;
  applied_height_ = height;
  // A fractional layout adjustment or allocation change may produce the same
  // integer union. Do not recreate protocol regions or schedule another frame.
  if (region_set_ && applied_regions_ == rectangles) return;
  wl_region* region = nullptr;
  if (!rectangles.empty()) {
    region = wl_compositor_create_region(compositor_);
    for (const auto& rect : rectangles) {
      wl_region_add(region, rect.x, rect.y, rect.width, rect.height);
    }
  }
  ext_background_effect_surface_v1_set_blur_region(effect_, region);
  if (region != nullptr) wl_region_destroy(region);
  // A property-only commit can leave the last scanout contents untouched.
  // Damage both the old and new mask, including pixels the effect no longer
  // covers. GDK still owns the commit and its pending Flutter buffer.
  cairo_region_t* repaint = cairo_region_create();
  for (const auto* mask : {&applied_regions_, &rectangles}) {
    for (const auto& rect : *mask) {
      const cairo_rectangle_int_t area = {rect.x, rect.y, rect.width, rect.height};
      cairo_region_union_rectangle(repaint, &area);
    }
  }
  if (!cairo_region_is_empty(repaint)) {
    cairo_rectangle_int_t bounds;
    cairo_region_get_extents(repaint, &bounds);
    wl_surface_damage(surface_, bounds.x, bounds.y, bounds.width, bounds.height);
    GdkWindow* window = gtk_widget_get_window(widget_);
    if (window != nullptr) gdk_window_invalidate_region(window, repaint, TRUE);
  }
  cairo_region_destroy(repaint);
  applied_regions_ = std::move(rectangles);
  region_set_ = true;
  // GDK owns surface commits, including pending buffers and frame callbacks.
  // Invalidate the widget so its next normal commit applies this double-buffered
  // state even when Flutter is otherwise idle.
  gtk_widget_queue_draw(widget_);
  wl_display_flush(display_);
}

void BackgroundEffect::ReleaseSurface() {
  if (effect_ != nullptr) {
    ext_background_effect_surface_v1_destroy(effect_);
    // The cached surface is borrowed: GDK may already have destroyed it during
    // unmap, unrealize or remap. Only our effect proxy can be used here. If the
    // widget is still mapped, GDK's next draw commits the cleared effect state.
    if (widget_ != nullptr && gtk_widget_get_mapped(widget_)) gtk_widget_queue_draw(widget_);
  }
  effect_ = nullptr; surface_ = nullptr; applied_ = false;
  applied_regions_.clear(); region_set_ = false;
  applied_width_ = -1; applied_height_ = -1;
}

void BackgroundEffect::Global(void* data, wl_registry* registry, uint32_t name,
                              const char* interface, uint32_t version) {
  auto* self = static_cast<BackgroundEffect*>(data);
  if (std::strcmp(interface, wl_compositor_interface.name) == 0 && self->compositor_ == nullptr) {
    self->compositor_name_ = name;
    self->compositor_ = static_cast<wl_compositor*>(wl_registry_bind(
        registry, name, &wl_compositor_interface, std::min(version, 4u)));
  } else if (std::strcmp(interface, ext_background_effect_manager_v1_interface.name) == 0 &&
             self->manager_ == nullptr) {
    self->manager_name_ = name;
    self->manager_ = static_cast<ext_background_effect_manager_v1*>(wl_registry_bind(
        registry, name, &ext_background_effect_manager_v1_interface, 1));
    static const ext_background_effect_manager_v1_listener listener = {Capabilities};
    ext_background_effect_manager_v1_add_listener(self->manager_, &listener, self);
  }
  self->Refresh();
}

void BackgroundEffect::GlobalRemoved(void* data, wl_registry*, uint32_t name) {
  auto* self = static_cast<BackgroundEffect*>(data);
  if (name != self->manager_name_ && name != self->compositor_name_) return;
  self->ReleaseSurface();
  if (name == self->manager_name_) {
    ext_background_effect_manager_v1_destroy(self->manager_);
    self->manager_ = nullptr; self->manager_name_ = 0; self->blur_capability_ = false;
  }
  if (name == self->compositor_name_) {
    wl_compositor_destroy(self->compositor_);
    self->compositor_ = nullptr; self->compositor_name_ = 0;
  }
}

void BackgroundEffect::Capabilities(void* data, ext_background_effect_manager_v1*, uint32_t flags) {
  auto* self = static_cast<BackgroundEffect*>(data);
  self->blur_capability_ = (flags & EXT_BACKGROUND_EFFECT_MANAGER_V1_CAPABILITY_BLUR) != 0;
  if (!self->blur_capability_) self->ReleaseSurface();
  self->Refresh();
}
void BackgroundEffect::Realized(GtkWidget*, gpointer data) {
  static_cast<BackgroundEffect*>(data)->Refresh();
}
void BackgroundEffect::Unrealized(GtkWidget*, gpointer data) {
  static_cast<BackgroundEffect*>(data)->ReleaseSurface();
}
void BackgroundEffect::Mapped(GtkWidget*, gpointer data) {
  static_cast<BackgroundEffect*>(data)->Refresh();
}
void BackgroundEffect::Unmapped(GtkWidget*, gpointer data) {
  static_cast<BackgroundEffect*>(data)->ReleaseSurface();
}
