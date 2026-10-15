// SPDX-License-Identifier: GPL-3.0-or-later
// Standalone GTK layer-shell QA. Compile with the production background_effect.cc
// and generated ext-background-effect-v1 protocol; no Flutter engine is needed.
#include <gtk/gtk.h>
#include <gtk-layer-shell.h>
#include <gdk/gdkwayland.h>

#include <functional>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

#include "background_effect.h"

namespace {
constexpr int kHeight = 32;
const char* result_path = nullptr;
int draw_count = 0;

struct Phase {
  std::string name;
  int allocated_width;
  int allocated_height;
  int applied_width;
  int applied_height;
  bool available;
  bool requested;
  bool realized;
  bool mapped;
  int draw_count;
  std::vector<realand_shell::BlurRect> applied_regions;
};
std::vector<Phase> phases;

std::string JsonString(const std::string& value) {
  std::ostringstream result;
  result << '"';
  for (const unsigned char ch : value) {
    switch (ch) {
      case '"': result << "\\\""; break;
      case '\\': result << "\\\\"; break;
      case '\n': result << "\\n"; break;
      case '\r': result << "\\r"; break;
      case '\t': result << "\\t"; break;
      default:
        if (ch < 0x20) {
          const char hex[] = "0123456789abcdef";
          result << "\\u00" << hex[ch >> 4] << hex[ch & 15];
        } else {
          result << ch;
        }
    }
  }
  result << '"';
  return result.str();
}

bool WriteResult(bool passed, const std::string& message) {
  std::ostringstream result;
  result << "{\n  \"passed\": " << (passed ? "true" : "false")
         << ",\n  \"message\": " << JsonString(message)
         << ",\n  \"scope\": \"real GTK layer allocation and production helper lifecycle;"
            " compositor pixel rendering is verified separately\",\n"
         << "  \"unchangedRegion\": {\"x\":10,\"y\":4,\"width\":100,"
            "\"height\":20,\"radius\":0},\n  \"phases\": [\n";
  for (size_t i = 0; i < phases.size(); ++i) {
    const auto& phase = phases[i];
    result << "    {\"name\":" << JsonString(phase.name)
           << ",\"allocatedWidth\":" << phase.allocated_width
           << ",\"allocatedHeight\":" << phase.allocated_height
           << ",\"appliedWidth\":" << phase.applied_width
           << ",\"appliedHeight\":" << phase.applied_height
           << ",\"available\":" << (phase.available ? "true" : "false")
           << ",\"requested\":" << (phase.requested ? "true" : "false")
           << ",\"realized\":" << (phase.realized ? "true" : "false")
           << ",\"mapped\":" << (phase.mapped ? "true" : "false")
           << ",\"drawCount\":" << phase.draw_count
           << ",\"appliedRegions\":[";
    for (size_t j = 0; j < phase.applied_regions.size(); ++j) {
      const auto& rect = phase.applied_regions[j];
      result << "{\"x\":" << rect.x << ",\"y\":" << rect.y
             << ",\"width\":" << rect.width << ",\"height\":" << rect.height
             << '}' << (j + 1 == phase.applied_regions.size() ? "" : ",");
    }
    result << "]}" << (i + 1 == phases.size() ? "\n" : ",\n");
  }
  result << "  ]\n}\n";
  GError* error = nullptr;
  const auto text = result.str();
  if (!g_file_set_contents(result_path, text.c_str(), text.size(), &error)) {
    g_printerr("Cannot write private background-effect QA result: %s\n",
               error->message);
    g_clear_error(&error);
    return false;
  }
  return true;
}

void Check(bool condition, const std::string& message) {
  if (!condition) throw std::runtime_error(message);
}

void WaitUntil(const char* phase, const std::function<bool()>& ready) {
  const gint64 deadline = g_get_monotonic_time() + 5 * G_TIME_SPAN_SECOND;
  while (!ready()) {
    while (g_main_context_iteration(nullptr, FALSE)) {}
    if (ready()) return;
    Check(g_get_monotonic_time() < deadline,
          std::string("Timed out waiting for ") + phase);
    g_usleep(1000);
  }
}

void Record(const char* name, GtkWidget* widget,
            const BackgroundEffect& effect) {
  phases.push_back({name, gtk_widget_get_allocated_width(widget),
                    gtk_widget_get_allocated_height(widget),
                    effect.AppliedWidth(), effect.AppliedHeight(),
                    effect.Available(), effect.Requested(),
                    gtk_widget_get_realized(widget) != FALSE,
                    gtk_widget_get_mapped(widget) != FALSE,
                    draw_count,
                    effect.AppliedRegions()});
}

void SizeAllocated(GtkWidget*, GtkAllocation*, gpointer data) {
  // This is the same production path as WindowController::DockSizeAllocated.
  static_cast<BackgroundEffect*>(data)->Refresh();
}

gboolean Draw(GtkWidget*, cairo_t* cr, gpointer) {
  ++draw_count;
  cairo_set_operator(cr, CAIRO_OPERATOR_SOURCE);
  cairo_set_source_rgba(cr, 0.2, 0.3, 0.5, 0.15);
  cairo_paint(cr);
  return FALSE;
}

struct DestroyWindow {
  void operator()(GtkWidget* widget) const {
    gtk_widget_destroy(widget);
    g_object_unref(widget);
  }
};

struct AllocationObserver {
  GtkWidget* widget;
  gulong handler;
  void Disconnect() {
    if (handler != 0) g_signal_handler_disconnect(widget, handler);
    handler = 0;
  }
  ~AllocationObserver() { Disconnect(); }
};

void SetSize(GtkWidget* widget, int width) {
  gtk_widget_set_size_request(widget, width, kHeight);
  gtk_window_resize(GTK_WINDOW(widget), width, kHeight);
}

void Run() {
  Check(GDK_IS_WAYLAND_DISPLAY(gdk_display_get_default()),
        "GTK did not connect to the private Wayland display");
  Check(gtk_layer_is_supported(), "Private compositor lacks layer-shell");
  std::unique_ptr<GtkWidget, DestroyWindow> window(
      gtk_window_new(GTK_WINDOW_TOPLEVEL));
  g_object_ref_sink(window.get());
  gtk_layer_init_for_window(GTK_WINDOW(window.get()));
  gtk_layer_set_namespace(GTK_WINDOW(window.get()),
                          "realand-background-effect-allocation-qa");
  gtk_layer_set_layer(GTK_WINDOW(window.get()), GTK_LAYER_SHELL_LAYER_TOP);
  gtk_layer_set_keyboard_mode(GTK_WINDOW(window.get()),
                              GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
  gtk_layer_set_exclusive_zone(GTK_WINDOW(window.get()), 0);
  gtk_layer_set_anchor(GTK_WINDOW(window.get()), GTK_LAYER_SHELL_EDGE_TOP, TRUE);
  gtk_layer_set_anchor(GTK_WINDOW(window.get()), GTK_LAYER_SHELL_EDGE_LEFT, TRUE);
  gtk_window_set_decorated(GTK_WINDOW(window.get()), FALSE);
  gtk_widget_set_app_paintable(window.get(), TRUE);
  if (GdkVisual* visual = gdk_screen_get_rgba_visual(
          gtk_widget_get_screen(window.get()))) {
    gtk_widget_set_visual(window.get(), visual);
  }
  GtkWidget* content = gtk_drawing_area_new();
  g_signal_connect(content, "draw", G_CALLBACK(Draw), nullptr);
  gtk_container_add(GTK_CONTAINER(window.get()), content);

  BackgroundEffect effect(window.get());
  AllocationObserver observer{
      window.get(), g_signal_connect(window.get(), "size-allocate",
                                     G_CALLBACK(SizeAllocated), &effect)};
  SetSize(window.get(), 60);
  effect.SetRegion(true, 10, 4, 100, 20);
  Check(effect.Requested() && effect.AppliedWidth() == -1 &&
            effect.AppliedHeight() == -1 && effect.AppliedRegions().empty(),
        "A request before first map touched an absent Wayland surface");
  Record("enabled-before-first-map", window.get(), effect);
  gtk_widget_show_all(window.get());
  WaitUntil("initial 60px allocation and protocol discovery", [&] {
    return effect.Available() && effect.AppliedWidth() == 60 &&
           effect.AppliedHeight() == kHeight && draw_count > 0;
  });
  Check(effect.Requested(), "Initial region request was lost");
  Check(effect.AppliedRegions() == std::vector<realand_shell::BlurRect>{{10, 4, 50, 20}},
        "Initial blur was not clipped to the 60px allocation");
  Record("initial-clipped-to-60", window.get(), effect);

  // Intentionally repeat the exact same request before the configure response.
  // A cached SetRegion cannot assume that the old clipped region remains valid.
  const int draws_before_resize = draw_count;
  SetSize(window.get(), 140);
  effect.SetRegion(true, 10, 4, 100, 20);
  WaitUntil("unchanged region re-applied at 140px", [&] {
    return gtk_widget_get_allocated_width(window.get()) == 140 &&
           effect.AppliedWidth() == 140 && effect.AppliedHeight() == kHeight &&
           draw_count > draws_before_resize;
  });
  Check(effect.Available() && effect.Requested(),
        "Resize lost the blur capability or request");
  const std::vector<realand_shell::BlurRect> full_region{{10, 4, 100, 20}};
  Check(effect.AppliedRegions() == full_region,
        "Resize did not restore the previously clipped blur region");
  Record("unchanged-region-after-resize-to-140", window.get(), effect);

  const int draws_before_clear = draw_count;
  effect.SetRegion(false, 10, 4, 100, 20);
  Check(!effect.Requested(), "Switch-to-Material clear retained blur request");
  Check(effect.AppliedWidth() == 140 && effect.AppliedHeight() == kHeight,
        "Clear was not applied to the current allocation");
  Check(effect.AppliedRegions().empty(), "Material clear retained an applied region");
  WaitUntil("GDK draw applying the Material clear", [&] {
    return draw_count > draws_before_clear;
  });
  Record("material-clear", window.get(), effect);

  gtk_widget_hide(window.get());
  gtk_widget_unrealize(window.get());
  Check(!gtk_widget_get_realized(window.get()), "Layer window stayed realized");
  Check(effect.AppliedWidth() == -1 && effect.AppliedHeight() == -1,
        "Unrealize retained dimensions from the destroyed protocol surface");
  Check(!effect.Requested(), "Unrealize changed the cleared desired state");
  Record("unrealized-cleared", window.get(), effect);

  gtk_widget_show_all(window.get());
  WaitUntil("cleared surface remap", [&] {
    return gtk_widget_get_mapped(window.get()) && effect.Available() &&
           gtk_widget_get_allocated_width(window.get()) == 140;
  });
  Check(!effect.Requested(), "Remapping resurrected a cleared blur request");
  Check(effect.AppliedWidth() == -1 && effect.AppliedHeight() == -1,
        "Remapping a cleared request created an unnecessary effect surface");
  Record("remapped-cleared", window.get(), effect);

  effect.SetRegion(true, 10, 4, 100, 20);
  WaitUntil("Mac blur after cleared remap", [&] {
    return effect.Requested() && effect.AppliedWidth() == 140 &&
           effect.AppliedHeight() == kHeight;
  });
  Record("mac-region-reenabled-after-remap", window.get(), effect);

  // Also remap while enabled: the desired rectangle must survive, while the
  // old wl_surface/effect object and its allocation cache must be discarded.
  gtk_widget_hide(window.get());
  gtk_widget_unrealize(window.get());
  Check(effect.Requested() && effect.AppliedWidth() == -1 &&
            effect.AppliedHeight() == -1,
        "Enabled unrealize did not release only the applied surface state");
  Record("unrealized-enabled", window.get(), effect);
  gtk_widget_show_all(window.get());
  WaitUntil("enabled surface remap", [&] {
    return gtk_widget_get_mapped(window.get()) && effect.Available() &&
           effect.Requested() && effect.AppliedWidth() == 140 &&
           effect.AppliedHeight() == kHeight;
  });
  Record("remapped-enabled", window.get(), effect);

  // Match the real style switch: ConfigureDockWindow hides the Dock, then
  // UpdateBackgroundEffects immediately clears the desired region. GTK can
  // keep this widget realized while replacing or destroying its wl_surface.
  for (int iteration = 0; iteration < 20; ++iteration) {
    const std::string suffix = "-" + std::to_string(iteration + 1);
    gtk_widget_hide(window.get());
    Check(!gtk_widget_get_mapped(window.get()), "Hidden Dock stayed mapped");
    effect.SetRegion(false, 10, 4, 100, 20);
    effect.Refresh();
    Check(!effect.Requested() && effect.AppliedWidth() == -1 &&
              effect.AppliedHeight() == -1 && effect.AppliedRegions().empty(),
          "Hide followed by Material clear retained a dead surface or region");
    Record(("material-hidden" + suffix).c_str(), window.get(), effect);

    const int draws_before_material = draw_count;
    gtk_widget_show_all(window.get());
    WaitUntil("Material show after hidden clear", [&] {
      return gtk_widget_get_mapped(window.get()) && effect.Available() &&
             gtk_widget_get_allocated_width(window.get()) == 140 &&
             draw_count > draws_before_material;
    });
    effect.Refresh();
    Check(!effect.Requested() && effect.AppliedWidth() == -1 &&
              effect.AppliedHeight() == -1 && effect.AppliedRegions().empty(),
          "Showing Material recreated a cleared effect");
    Record(("material-remapped" + suffix).c_str(), window.get(), effect);

    const int draws_before_mac = draw_count;
    effect.SetRegion(true, 10, 4, 100, 20);
    WaitUntil("macOS effect restored after Material", [&] {
      return effect.Requested() && effect.AppliedWidth() == 140 &&
             effect.AppliedHeight() == kHeight && effect.AppliedRegions() == full_region &&
             draw_count > draws_before_mac;
    });
    Record(("mac-reenabled" + suffix).c_str(), window.get(), effect);

    // Remapping an enabled effect must preserve its desired region but never
    // reuse an inert effect object, including if a proxy address is recycled.
    gtk_widget_hide(window.get());
    effect.Refresh();
    Check(effect.Requested() && effect.AppliedWidth() == -1 &&
              effect.AppliedHeight() == -1 && effect.AppliedRegions().empty(),
          "Enabled hide retained applied state from its old Wayland surface");
    Record(("mac-hidden-enabled" + suffix).c_str(), window.get(), effect);
    const int draws_before_enabled_remap = draw_count;
    gtk_widget_show_all(window.get());
    WaitUntil("enabled hide/show remap", [&] {
      return gtk_widget_get_mapped(window.get()) && effect.Available() &&
             effect.Requested() && effect.AppliedWidth() == 140 &&
             effect.AppliedHeight() == kHeight && effect.AppliedRegions() == full_region &&
             draw_count > draws_before_enabled_remap;
    });
    Record(("mac-remapped-enabled" + suffix).c_str(), window.get(), effect);
  }
  effect.SetRegion(false, 10, 4, 100, 20);
  Check(!effect.Requested(), "Final cleanup did not clear the effect");
  Record("final-clear", window.get(), effect);

  const int draws_before_destroy = draw_count;
  effect.SetRegion(true, 10, 4, 100, 20);
  WaitUntil("enabled effect before window destroy", [&] {
    return effect.Requested() && effect.AppliedWidth() == 140 &&
           effect.AppliedHeight() == kHeight && effect.AppliedRegions() == full_region &&
           draw_count > draws_before_destroy;
  });
  observer.Disconnect();
  // Keep the production helper alive through both GTK destroy and final object
  // disposal. Its callbacks and weak pointer must release only owned proxies.
  gtk_widget_destroy(window.get());
  Check(!gtk_widget_get_mapped(window.get()) && !gtk_widget_get_realized(window.get()) &&
            effect.Requested() && effect.AppliedWidth() == -1 &&
            effect.AppliedHeight() == -1 && effect.AppliedRegions().empty(),
        "Destroying the window retained applied surface state");
  Record("destroyed-with-enabled-effect", window.get(), effect);
  window.reset();
  effect.Refresh();
  Check(effect.Requested() && effect.AppliedWidth() == -1 &&
            effect.AppliedHeight() == -1 && effect.AppliedRegions().empty(),
        "Disposing the widget resurrected an enabled effect");
}

bool DifferentEnvironment(const char* key, const char* parent_key) {
  const char* current = g_getenv(key);
  const char* parent = g_getenv(parent_key);
  return current && *current && parent && *parent &&
         g_strcmp0(current, parent) != 0;
}
}  // namespace

int main(int argc, char** argv) {
  result_path = g_getenv("REALAND_BACKGROUND_EFFECT_QA_RESULT");
  if (g_strcmp0(g_getenv("REALAND_SHELL_ISOLATED_TEST"), "1") != 0 ||
      !DifferentEnvironment("WAYLAND_DISPLAY", "REALAND_SHELL_PARENT_WAYLAND_DISPLAY") ||
      !DifferentEnvironment("DBUS_SESSION_BUS_ADDRESS", "REALAND_SHELL_PARENT_DBUS_SESSION_BUS_ADDRESS") ||
      !DifferentEnvironment("REALAND_SOCKET", "REALAND_SHELL_PARENT_SOCKET") ||
      !result_path || !g_path_is_absolute(result_path)) {
    g_printerr("Refusing non-isolated background-effect allocation QA environment\n");
    return 2;
  }
  // Select Wayland explicitly so GTK cannot silently fall back to the parent X
  // display. All isolation checks run before initializing any display or bus.
  g_setenv("GDK_BACKEND", "wayland", TRUE);
  if (!gtk_init_check(&argc, &argv)) {
    WriteResult(false, "Cannot initialize GTK on the private Wayland display");
    return 1;
  }
  try {
    Run();
    return WriteResult(true, "All allocation and lifecycle checks passed") ? 0 : 1;
  } catch (const std::exception& error) {
    g_printerr("Background-effect allocation QA failed: %s\n", error.what());
    WriteResult(false, error.what());
    return 1;
  }
}
