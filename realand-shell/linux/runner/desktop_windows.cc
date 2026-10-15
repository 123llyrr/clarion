// SPDX-License-Identifier: GPL-3.0-or-later
#include "desktop_windows.h"

#include <flutter_linux/flutter_linux.h>
#include <gdk/gdkwayland.h>
#include <gtk-layer-shell.h>
#include <wayland-client.h>

#include <algorithm>
#include <cstring>
#include <string>
#include <utility>
#include <vector>

#include "app_icon_lookup.h"
#include "flutter/generated_plugin_registrant.h"

namespace {
constexpr int kCornerPassthrough = 8;

// GTK 3 exposes its wl_output, but not the connector name. Bind an independent
// XDG-output v2 object to that output rather than replacing GTK's own listener.
// These wire descriptions follow the public XDG-output protocol:
// https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/unstable/xdg-output/xdg-output-unstable-v1.xml
extern const wl_interface kXdgOutputInterface;
const wl_interface* kXdgOutputTypes[] = {&kXdgOutputInterface,
                                        &wl_output_interface};
const wl_message kXdgManagerRequests[] = {
    {"destroy", "", nullptr},
    {"get_xdg_output", "no", kXdgOutputTypes},
};
const wl_interface kXdgManagerInterface = {
    "zxdg_output_manager_v1", 2, 2, kXdgManagerRequests, 0, nullptr};
const wl_message kXdgOutputRequests[] = {{"destroy", "", nullptr}};
const wl_message kXdgOutputEvents[] = {
    {"logical_position", "ii", nullptr},
    {"logical_size", "ii", nullptr},
    {"done", "", nullptr},
    {"name", "2s", nullptr},
    {"description", "2s", nullptr},
};
const wl_interface kXdgOutputInterface = {
    "zxdg_output_v1", 2, 1, kXdgOutputRequests, 5, kXdgOutputEvents};

// Metadata-only GTK v1, matching resources/gtk-shell.xml. GDK's convenience
// setter publishes only once, so keep our own object for live style changes.
// Neither v1 interface has a destructor request; use wl_proxy_destroy below.
extern const wl_interface kGtkSurfaceInterface;
const wl_interface* kGtkSurfaceTypes[] = {&kGtkSurfaceInterface, &wl_surface_interface};
const wl_interface* kGtkBellTypes[] = {&kGtkSurfaceInterface};
const wl_message kGtkShellRequests[] = {
    {"get_gtk_surface", "no", kGtkSurfaceTypes},
    {"set_startup_id", "?s", nullptr},
    {"system_bell", "?o", kGtkBellTypes},
};
const wl_message kGtkShellEvents[] = {{"capabilities", "u", nullptr}};
const wl_interface kGtkShellInterface = {
    "gtk_shell1", 1, 3, kGtkShellRequests, 1, kGtkShellEvents};
const wl_message kGtkSurfaceRequests[] = {
    {"set_dbus_properties", "?s?s?s?s?s?s", nullptr},
    {"set_modal", "", nullptr}, {"unset_modal", "", nullptr},
    {"present", "u", nullptr},
};
const wl_message kGtkSurfaceEvents[] = {{"configure", "a", nullptr}};
const wl_interface kGtkSurfaceInterface = {
    "gtk_surface1", 1, 4, kGtkSurfaceRequests, 1, kGtkSurfaceEvents};

void DestroyMetadata(wl_proxy** object) {
  if (*object == nullptr) return;
  wl_proxy_destroy(*object);
  *object = nullptr;
}

struct XdgOutputListener {
  void (*logical_position)(void*, wl_proxy*, int32_t, int32_t);
  void (*logical_size)(void*, wl_proxy*, int32_t, int32_t);
  void (*done)(void*, wl_proxy*);
  void (*name)(void*, wl_proxy*, const char*);
  void (*description)(void*, wl_proxy*, const char*);
};

void DestroyProtocol(wl_proxy** object) {
  if (*object == nullptr) return;
  wl_proxy_marshal_flags(*object, 0, nullptr, wl_proxy_get_version(*object),
                         WL_MARSHAL_FLAG_DESTROY);
  *object = nullptr;
}

void Respond(FlMethodCall* call, FlMethodResponse* response) {
  g_autoptr(GError) error = nullptr;
  if (!fl_method_call_respond(call, response, &error)) {
    g_warning("Unable to respond to Realand Desktop method: %s", error->message);
  }
}

gchar** DesktopArguments(gchar** source, int monitor_index,
                         const gchar* output_name, bool preview) {
  std::vector<std::string> options;
  for (gchar** argument = source; argument != nullptr && *argument != nullptr;
       ++argument) {
    if (strcmp(*argument, "--monitor") == 0) {
      if (*(argument + 1) != nullptr) ++argument;
      continue;
    }
    if (g_str_has_prefix(*argument, "--monitor=") ||
        g_str_has_prefix(*argument, "--output-name=") ||
        strcmp(*argument, "--preview") == 0 ||
        strcmp(*argument, "--desktop") == 0 ||
        strcmp(*argument, "--desktop-only") == 0 ||
        strcmp(*argument, "--desktop-preview") == 0 ||
        strcmp(*argument, "--no-desktop") == 0) {
      continue;
    }
    options.emplace_back(*argument);
  }
  options.emplace_back("--desktop");
  options.emplace_back("--no-notifications");
  options.emplace_back("--no-tray");
  options.emplace_back("--no-wallpaper");
  if (preview) {
    options.emplace_back("--preview");
    options.emplace_back("--desktop-preview");
  }
  if (monitor_index >= 0) {
    options.emplace_back("--monitor=" + std::to_string(monitor_index));
  }
  if (output_name != nullptr) {
    options.emplace_back(std::string("--output-name=") + output_name);
  }
  gchar** arguments = g_new0(gchar*, options.size() + 1);
  for (size_t index = 0; index < options.size(); ++index) {
    arguments[index] = g_strdup(options[index].c_str());
  }
  return arguments;
}

class DesktopWindow final {
 public:
  DesktopWindow(GtkApplication* application, GdkDisplay* display,
                GdkMonitor* monitor, gchar** dart_arguments, bool preview,
                const gchar* preview_output_name)
      : display_(display), monitor_(monitor), preview_(preview) {
    if (monitor_ != nullptr) {
      g_object_ref(monitor_);
      g_signal_connect(monitor_, "notify::geometry",
                        G_CALLBACK(MonitorChanged), this);
      g_signal_connect(monitor_, "notify::scale-factor",
                        G_CALLBACK(MonitorChanged), this);
    }
    if (preview_) {
      output_name_ = g_strdup(preview_output_name != nullptr
                                  ? preview_output_name : "Preview");
    }
    window_ = GTK_WINDOW(gtk_application_window_new(application));
    gtk_application_window_set_show_menubar(GTK_APPLICATION_WINDOW(window_), FALSE);
    g_object_add_weak_pointer(G_OBJECT(window_),
                             reinterpret_cast<gpointer*>(&window_));
    gtk_window_set_title(window_, preview_ ? "Realand Desktop Preview"
                                           : "Realand Desktop");
    gtk_window_set_default_size(window_, preview_ ? 1200 : 1,
                                preview_ ? 800 : 1);
    gtk_window_set_decorated(window_, preview_);
    gtk_window_set_skip_taskbar_hint(window_, !preview_);
    gtk_window_set_skip_pager_hint(window_, !preview_);
    gtk_widget_set_app_paintable(GTK_WIDGET(window_), TRUE);
    GdkVisual* visual = gdk_screen_get_rgba_visual(
        gtk_widget_get_screen(GTK_WIDGET(window_)));
    if (visual != nullptr) gtk_widget_set_visual(GTK_WIDGET(window_), visual);
    g_autoptr(GtkCssProvider) css = gtk_css_provider_new();
    gtk_css_provider_load_from_data(css,
        "window { background-color: transparent; background-image: none; }",
        -1, nullptr);
    gtk_style_context_add_provider(
        gtk_widget_get_style_context(GTK_WIDGET(window_)),
        GTK_STYLE_PROVIDER(css), GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);

    if (!preview_) {
      gtk_layer_init_for_window(window_);
      gtk_layer_set_namespace(window_, "realand-desktop");
      gtk_layer_set_monitor(window_, monitor_);
      ConfigureLayer();
    }
    arguments_ = DesktopArguments(dart_arguments, MonitorIndex(), output_name_,
                                  preview_);
    g_autoptr(FlDartProject) project = fl_dart_project_new();
    fl_dart_project_set_dart_entrypoint_arguments(project, arguments_);
    view_ = fl_view_new(project);
    g_object_add_weak_pointer(G_OBJECT(view_),
                             reinterpret_cast<gpointer*>(&view_));
    const GdkRGBA transparent = {0.0, 0.0, 0.0, 0.0};
    fl_view_set_background_color(view_, &transparent);
    gtk_container_add(GTK_CONTAINER(window_), GTK_WIDGET(view_));
    g_signal_connect(window_, "size-allocate", G_CALLBACK(SizeAllocated), this);
    g_signal_connect(window_, "style-updated", G_CALLBACK(StyleUpdated), this);
    // GtkApplicationWindow can publish its own metadata during map. Our
    // desktop identity must follow it, without touching the dialog's surface.
    g_signal_connect_after(window_, "map", G_CALLBACK(Mapped), this);
    g_signal_connect(window_, "unrealize", G_CALLBACK(Unrealized), this);
    g_signal_connect(window_, "destroy", G_CALLBACK(WindowDestroyed), this);
    g_signal_connect(view_, "first-frame", G_CALLBACK(FirstFrame), this);
    g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
    auto* messenger = fl_engine_get_binary_messenger(fl_view_get_engine(view_));
    channel_ = fl_method_channel_new(messenger, "realand.desktop/window",
                                    FL_METHOD_CODEC(codec));
    icon_channel_ = fl_method_channel_new(messenger, "realand.shell/window",
                                         FL_METHOD_CODEC(codec));
    fl_method_channel_set_method_call_handler(channel_, MethodCall, this, nullptr);
    fl_method_channel_set_method_call_handler(icon_channel_, IconMethodCall,
                                              this, nullptr);
    fl_register_plugins(FL_PLUGIN_REGISTRY(view_));
    gtk_widget_show(GTK_WIDGET(view_));
    gtk_widget_realize(GTK_WIDGET(view_));
  }

  ~DesktopWindow() {
    DetachChannels();
    DestroyInteractionView();
    DestroyMetadata(&gtk_surface_);
    DestroyProtocol(&output_info_);
    if (monitor_ != nullptr) {
      g_signal_handlers_disconnect_by_data(monitor_, this);
      g_clear_object(&monitor_);
    }
    if (view_ != nullptr) {
      g_signal_handlers_disconnect_by_data(view_, this);
      g_object_remove_weak_pointer(G_OBJECT(view_),
                                  reinterpret_cast<gpointer*>(&view_));
      view_ = nullptr;
    }
    if (window_ != nullptr) {
      GtkWindow* window = window_;
      g_signal_handlers_disconnect_by_data(window, this);
      g_object_remove_weak_pointer(G_OBJECT(window),
                                  reinterpret_cast<gpointer*>(&window_));
      window_ = nullptr;
      gtk_widget_destroy(GTK_WIDGET(window));
    }
    g_clear_pointer(&arguments_, g_strfreev);
    g_clear_pointer(&output_name_, g_free);
  }

  GdkMonitor* monitor() const { return monitor_; }

  void BindGtkMetadata(wl_proxy* shell) {
    gtk_shell_ = shell;
    PublishShortcutStyle();
  }

  void BindOutputInfo(wl_proxy* manager) {
    if (preview_ || output_info_ != nullptr || monitor_ == nullptr ||
        !GDK_IS_WAYLAND_MONITOR(monitor_)) return;
    wl_output* output = gdk_wayland_monitor_get_wl_output(monitor_);
    if (output == nullptr) return;
    output_info_ = wl_proxy_marshal_flags(
        manager, 1, &kXdgOutputInterface, wl_proxy_get_version(manager), 0,
        nullptr, output);
    if (output_info_ == nullptr) return;
    static const XdgOutputListener listener = {
        OutputPosition, OutputSize, OutputDone, OutputName, OutputDescription};
    wl_proxy_add_listener(output_info_,
        reinterpret_cast<void (**)(void)>(
            const_cast<XdgOutputListener*>(&listener)), this);
  }

  void NotifyOutputChanged() {
    if (closed_ || channel_ == nullptr) return;
    fl_method_channel_invoke_method(channel_, "outputChanged", nullptr, nullptr,
                                    nullptr, nullptr);
  }

 private:
  void PublishShortcutStyle() {
    if (preview_ || closed_ || window_ == nullptr || gtk_shell_ == nullptr) return;
    GdkWindow* window = gtk_widget_get_window(GTK_WIDGET(window_));
    if (window == nullptr || !GDK_IS_WAYLAND_WINDOW(window)) return;
    wl_surface* surface = gdk_wayland_window_get_wl_surface(window);
    if (surface == nullptr) return;
    if (surface != gtk_metadata_owner_) {
      DestroyMetadata(&gtk_surface_);
      gtk_metadata_owner_ = surface;
    }
    if (gtk_surface_ == nullptr) {
      gtk_surface_ = wl_proxy_marshal_flags(gtk_shell_, 0, &kGtkSurfaceInterface,
          1, 0, nullptr, surface);
      if (gtk_surface_ == nullptr) return;
      static const struct {
        void (*configure)(void*, wl_proxy*, wl_array*);
      } listener = {[](void*, wl_proxy*, wl_array*) {}};
      wl_proxy_add_listener(gtk_surface_,
          reinterpret_cast<void (**)(void)>(const_cast<void*>(
              static_cast<const void*>(&listener))), this);
    }
    wl_proxy_marshal_flags(gtk_surface_, 0, nullptr, 1, 0,
        macos_shortcuts_ ? "io.github.realand.Desktop.MacOS"
                         : "io.github.realand.Desktop.Material",
        nullptr, nullptr, nullptr, nullptr, nullptr);
    wl_display_flush(gdk_wayland_display_get_wl_display(display_));
  }

  int MonitorIndex() const {
    for (int index = 0; index < gdk_display_get_n_monitors(display_); ++index) {
      if (gdk_display_get_monitor(display_, index) == monitor_) return index;
    }
    return -1;
  }

  GdkRectangle MonitorGeometry() const {
    GdkRectangle geometry = {0, 0, 0, 0};
    if (monitor_ != nullptr) gdk_monitor_get_geometry(monitor_, &geometry);
    return geometry;
  }

  void ConfigureLayer() {
    if (preview_ || closed_ || window_ == nullptr) return;
    for (int edge = 0; edge < GTK_LAYER_SHELL_EDGE_ENTRY_NUMBER; ++edge) {
      gtk_layer_set_anchor(window_, static_cast<GtkLayerShellEdge>(edge), TRUE);
    }
    // The persistent file grid never changes stacking order. Menus and dialogs
    // have a separate view, so opening one cannot hide the desktop or put files
    // that were covered by an application in front of that application.
    gtk_layer_set_layer(window_, GTK_LAYER_SHELL_LAYER_BOTTOM);
    // -1 adds no reservation and spans the monitor even when the bar reserves
    // an edge. Dart applies the bar padding once to its desktop grid.
    gtk_layer_set_exclusive_zone(window_, -1);
    gtk_layer_set_keyboard_mode(window_, gtk_layer_get_protocol_version() >= 4
        ? GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND
        : GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
    gtk_widget_set_size_request(GTK_WIDGET(window_), -1, -1);
    gtk_window_resize(window_, 1, 1);
  }

  void UpdateSurfaceRegions() {
    if (closed_ || window_ == nullptr) return;
    GdkWindow* surface = gtk_widget_get_window(GTK_WIDGET(window_));
    if (surface == nullptr) return;
    gdk_window_set_opaque_region(surface, nullptr);
    if (preview_) {
      gdk_window_input_shape_combine_region(surface, nullptr, 0, 0);
      return;
    }
    const int width = gtk_widget_get_allocated_width(GTK_WIDGET(window_));
    const int height = gtk_widget_get_allocated_height(GTK_WIDGET(window_));
    const cairo_rectangle_int_t full = {0, 0, width, height};
    cairo_region_t* input = cairo_region_create_rectangle(&full);
    const int corner = std::min(kCornerPassthrough,
                                std::min(width / 2, height / 2));
    for (const int x : {0, width - corner}) {
      for (const int y : {0, height - corner}) {
        const cairo_rectangle_int_t cutout = {x, y, corner, corner};
        cairo_region_subtract_rectangle(input, &cutout);
      }
    }
    gdk_window_input_shape_combine_region(surface, input, 0, 0);
    cairo_region_destroy(input);
  }

  bool SetOverlay(bool overlay) {
    if (closed_ || window_ == nullptr) return false;
    if (preview_) {
      overlay_ = overlay;
      return true;
    }
    if (overlay && !EnsureInteractionView()) return false;
    if (overlay_ == overlay) return true;
    overlay_ = overlay;
    ConfigureInteractionWindow();
    if (!overlay && view_ != nullptr) {
      // Restore GTK's focused child when the separate popup gives up its grab.
      // Wayland seat focus is restored by the compositor; this never raises the
      // desktop or requests focus from an unrelated application.
      gtk_widget_grab_focus(GTK_WIDGET(view_));
    }
    return true;
  }

  bool EnsureInteractionView() {
    if (preview_) return true;
    if (closed_ || window_ == nullptr || view_ == nullptr) return false;
    if (interaction_window_ != nullptr && interaction_view_ != nullptr) {
      return true;
    }
    DestroyInteractionView();
    GtkApplication* application = gtk_window_get_application(window_);
    if (application == nullptr) return false;
    GtkWindow* window = GTK_WINDOW(gtk_application_window_new(application));
    gtk_application_window_set_show_menubar(GTK_APPLICATION_WINDOW(window), FALSE);
    gtk_window_set_title(window, "Realand Desktop Interaction");
    gtk_window_set_decorated(window, FALSE);
    gtk_window_set_skip_taskbar_hint(window, TRUE);
    gtk_window_set_skip_pager_hint(window, TRUE);
    gtk_widget_set_app_paintable(GTK_WIDGET(window), TRUE);
    GdkVisual* visual = gdk_screen_get_rgba_visual(
        gtk_widget_get_screen(GTK_WIDGET(window)));
    if (visual != nullptr) gtk_widget_set_visual(GTK_WIDGET(window), visual);
    g_autoptr(GtkCssProvider) css = gtk_css_provider_new();
    gtk_css_provider_load_from_data(css,
        "window { background-color: transparent; background-image: none; }",
        -1, nullptr);
    gtk_style_context_add_provider(
        gtk_widget_get_style_context(GTK_WIDGET(window)), GTK_STYLE_PROVIDER(css),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    gtk_layer_init_for_window(window);
    gtk_layer_set_namespace(window, "realand-desktop-interaction");
    gtk_layer_set_monitor(window, monitor_);

    // Both views belong to one engine. The desktop's existing channels and
    // plugin registrations serve the interaction view as well.
    FlView* view = fl_view_new_for_engine(fl_view_get_engine(view_));
    if (view == nullptr || fl_view_get_id(view) < 0) {
      if (view != nullptr) {
        g_object_ref_sink(view);
        gtk_widget_destroy(GTK_WIDGET(view));
        g_object_unref(view);
      }
      gtk_widget_destroy(GTK_WIDGET(window));
      return false;
    }
    const GdkRGBA transparent = {0.0, 0.0, 0.0, 0.0};
    fl_view_set_background_color(view, &transparent);
    gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(view));
    interaction_window_ = window;
    interaction_view_ = view;
    g_object_add_weak_pointer(G_OBJECT(interaction_window_),
        reinterpret_cast<gpointer*>(&interaction_window_));
    g_object_add_weak_pointer(G_OBJECT(interaction_view_),
        reinterpret_cast<gpointer*>(&interaction_view_));
    g_signal_connect(window, "size-allocate",
                      G_CALLBACK(InteractionSizeAllocated), this);
    g_signal_connect(window, "style-updated",
                      G_CALLBACK(InteractionStyleUpdated), this);
    g_signal_connect(window, "delete-event",
                      G_CALLBACK(InteractionDeleteEvent), this);
    g_signal_connect(window, "destroy",
                      G_CALLBACK(InteractionDestroyed), this);
    g_signal_connect(view, "first-frame",
                      G_CALLBACK(InteractionFirstFrame), this);
    interaction_gtk_draws_ = 0;
    g_signal_connect_after(view, "draw", G_CALLBACK(InteractionDrawn), this);
    ConfigureInteractionWindow();
    gtk_widget_show(GTK_WIDGET(view));
    gtk_widget_realize(GTK_WIDGET(view));
    if (!interaction_first_frame_) {
      interaction_ready_timeout_ =
          g_timeout_add_seconds(10, InteractionReadyTimeout, this);
    }
    return true;
  }

  void ConfigureInteractionWindow() {
    if (interaction_window_ == nullptr || closed_) return;
    for (int edge = 0; edge < GTK_LAYER_SHELL_EDGE_ENTRY_NUMBER; ++edge) {
      gtk_layer_set_anchor(interaction_window_,
                          static_cast<GtkLayerShellEdge>(edge), TRUE);
    }
    gtk_layer_set_layer(interaction_window_, GTK_LAYER_SHELL_LAYER_OVERLAY);
    gtk_layer_set_exclusive_zone(interaction_window_, -1);
    const bool active = overlay_ && interaction_first_frame_;
    gtk_layer_set_keyboard_mode(interaction_window_, active
        ? GTK_LAYER_SHELL_KEYBOARD_MODE_EXCLUSIVE
        : GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
    const GdkRectangle geometry = MonitorGeometry();
    gtk_widget_set_size_request(GTK_WIDGET(interaction_window_),
                               std::max(1, geometry.width),
                               std::max(1, geometry.height));
    gtk_window_resize(interaction_window_, std::max(1, geometry.width),
                      std::max(1, geometry.height));
    // Once its first transparent frame exists, keep this view mapped. GTK's
    // hide/show path unmaps the layer surface but retains Flutter's renderer.
    // Stable mapping avoids depending on a fresh GTK redraw after each remap
    // or output-size change and lets closing animations finish normally. The
    // interaction home explicitly clears its framebuffer, so an idle mapped
    // view is visually empty and can pass through all input instead of
    // recreating its Wayland role.
    UpdateInteractionRegions();
    if (interaction_first_frame_) {
      gtk_widget_show(GTK_WIDGET(interaction_window_));
      if (interaction_view_ != nullptr) {
        gtk_widget_queue_draw(GTK_WIDGET(interaction_view_));
      }
      gtk_widget_queue_draw(GTK_WIDGET(interaction_window_));
      GdkWindow* surface =
          gtk_widget_get_window(GTK_WIDGET(interaction_window_));
      if (surface != nullptr) gdk_window_invalidate_rect(surface, nullptr, TRUE);
      if (active && interaction_view_ != nullptr) {
        gtk_widget_grab_focus(GTK_WIDGET(interaction_view_));
      }
    } else {
      gtk_widget_hide(GTK_WIDGET(interaction_window_));
    }
    UpdateInteractionRegions();
  }

  void UpdateInteractionRegions() {
    if (interaction_window_ == nullptr || closed_) return;
    GdkWindow* surface =
        gtk_widget_get_window(GTK_WIDGET(interaction_window_));
    if (surface == nullptr) return;
    gdk_window_set_opaque_region(surface, nullptr);
    if (overlay_ && interaction_first_frame_) {
      // Modal dismissal owns the whole output, including its corners. The
      // desktop's normal corner passthrough must not activate a hot zone
      // through an open menu or confirmation dialog.
      gdk_window_input_shape_combine_region(surface, nullptr, 0, 0);
    } else {
      // A transparent, persistently mapped idle view must never intercept an
      // application, desktop icon, panel or hot corner underneath it.
      cairo_region_t* empty = cairo_region_create();
      gdk_window_input_shape_combine_region(surface, empty, 0, 0);
      cairo_region_destroy(empty);
    }
  }

  void DestroyInteractionView() {
    if (interaction_ready_timeout_ != 0) {
      g_source_remove(interaction_ready_timeout_);
      interaction_ready_timeout_ = 0;
    }
    if (interaction_view_ != nullptr) {
      g_signal_handlers_disconnect_by_data(interaction_view_, this);
      g_object_remove_weak_pointer(G_OBJECT(interaction_view_),
          reinterpret_cast<gpointer*>(&interaction_view_));
      interaction_view_ = nullptr;
    }
    if (interaction_window_ != nullptr) {
      GtkWindow* window = interaction_window_;
      g_signal_handlers_disconnect_by_data(window, this);
      gtk_layer_set_keyboard_mode(window, GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
      gtk_widget_hide(GTK_WIDGET(window));
      g_object_remove_weak_pointer(G_OBJECT(window),
          reinterpret_cast<gpointer*>(&interaction_window_));
      interaction_window_ = nullptr;
      gtk_widget_destroy(GTK_WIDGET(window));
    }
    interaction_first_frame_ = false;
  }

  void NotifyInteractionFailed(int64_t view_id, const gchar* reason) {
    if (closed_ || channel_ == nullptr) return;
    g_autoptr(FlValue) state = fl_value_new_map();
    fl_value_set_string_take(state, "viewId", fl_value_new_int(view_id));
    fl_value_set_string_take(state, "reason", fl_value_new_string(reason));
    fl_method_channel_invoke_method(channel_, "interactionViewFailed", state,
                                    nullptr, nullptr, nullptr);
  }

  void DetachChannels() {
    for (FlMethodChannel** channel : {&channel_, &icon_channel_}) {
      if (*channel == nullptr) continue;
      fl_method_channel_set_method_call_handler(*channel, nullptr, nullptr,
                                                nullptr);
      g_clear_object(channel);
    }
  }

  FlValue* WindowState() const {
    FlValue* state = fl_value_new_map();
    fl_value_set_string_take(state, "role", fl_value_new_string("desktop"));
    fl_value_set_string_take(state, "namespace",
                             fl_value_new_string("realand-desktop"));
    fl_value_set_string_take(state, "preview", fl_value_new_bool(preview_));
    fl_value_set_string_take(state, "shortcutStyle",
        fl_value_new_string(macos_shortcuts_ ? "macos" : "material"));
    fl_value_set_string_take(state, "shortcutMetadataPublished",
        fl_value_new_bool(gtk_surface_ != nullptr));
    fl_value_set_string_take(state, "overlay", fl_value_new_bool(overlay_));
    fl_value_set_string_take(state, "outputName", output_name_ == nullptr
        ? fl_value_new_null() : fl_value_new_string(output_name_));
    fl_value_set_string_take(state, "monitorIndex", fl_value_new_int(MonitorIndex()));
    fl_value_set_string_take(state, "width", fl_value_new_int(
        gtk_widget_get_allocated_width(GTK_WIDGET(window_))));
    fl_value_set_string_take(state, "height", fl_value_new_int(
        gtk_widget_get_allocated_height(GTK_WIDGET(window_))));
    fl_value_set_string_take(state, "scaleFactor", fl_value_new_int(
        gtk_widget_get_scale_factor(GTK_WIDGET(window_))));
    fl_value_set_string_take(state, "layer", fl_value_new_string(
        preview_ ? "Preview" : "Bottom"));
    fl_value_set_string_take(state, "exclusiveZone", fl_value_new_int(
        preview_ ? 0 : gtk_layer_get_exclusive_zone(window_)));
    const auto keyboard = preview_ ? GTK_LAYER_SHELL_KEYBOARD_MODE_NONE
                                   : gtk_layer_get_keyboard_mode(window_);
    fl_value_set_string_take(state, "keyboard", fl_value_new_string(
        keyboard == GTK_LAYER_SHELL_KEYBOARD_MODE_EXCLUSIVE ? "Exclusive"
        : keyboard == GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND ? "OnDemand"
                                                             : "None"));
    fl_value_set_string_take(state, "layerProtocolVersion", fl_value_new_int(
        preview_ ? 0 : gtk_layer_get_protocol_version()));
    fl_value_set_string_take(state, "focused",
                             fl_value_new_bool(gtk_window_is_active(window_)));
    fl_value_set_string_take(state, "cornerPassthrough",
                             fl_value_new_int(preview_ ? 0 : kCornerPassthrough));
    for (const auto& edge : {
        std::make_pair("anchorTop", GTK_LAYER_SHELL_EDGE_TOP),
        std::make_pair("anchorBottom", GTK_LAYER_SHELL_EDGE_BOTTOM),
        std::make_pair("anchorLeft", GTK_LAYER_SHELL_EDGE_LEFT),
        std::make_pair("anchorRight", GTK_LAYER_SHELL_EDGE_RIGHT)}) {
      fl_value_set_string_take(state, edge.first, fl_value_new_bool(
          !preview_ && gtk_layer_get_anchor(window_, edge.second)));
    }
    const GdkRectangle bounds = MonitorGeometry();
    FlValue* geometry = fl_value_new_map();
    fl_value_set_string_take(geometry, "x", fl_value_new_int(bounds.x));
    fl_value_set_string_take(geometry, "y", fl_value_new_int(bounds.y));
    fl_value_set_string_take(geometry, "width", fl_value_new_int(bounds.width));
    fl_value_set_string_take(geometry, "height", fl_value_new_int(bounds.height));
    fl_value_set_string_take(state, "monitorGeometry", geometry);
    FlValue* interaction = fl_value_new_map();
    fl_value_set_string_take(interaction, "viewId", interaction_view_ == nullptr
        ? fl_value_new_null() : fl_value_new_int(fl_view_get_id(interaction_view_)));
    fl_value_set_string_take(interaction, "namespace",
        fl_value_new_string("realand-desktop-interaction"));
    fl_value_set_string_take(interaction, "layer", fl_value_new_string("Overlay"));
    const bool exclusive = interaction_window_ != nullptr &&
        gtk_layer_get_keyboard_mode(interaction_window_) ==
            GTK_LAYER_SHELL_KEYBOARD_MODE_EXCLUSIVE;
    fl_value_set_string_take(interaction, "keyboard",
        fl_value_new_string(exclusive ? "Exclusive" : "None"));
    fl_value_set_string_take(interaction, "active", fl_value_new_bool(
        overlay_ && interaction_first_frame_ && interaction_window_ != nullptr));
    fl_value_set_string_take(interaction, "inputEmpty", fl_value_new_bool(
        !overlay_ || !interaction_first_frame_ || interaction_window_ == nullptr));
    fl_value_set_string_take(interaction, "visible", fl_value_new_bool(
        interaction_window_ != nullptr &&
        gtk_widget_get_visible(GTK_WIDGET(interaction_window_))));
    fl_value_set_string_take(interaction, "mapped", fl_value_new_bool(
        interaction_window_ != nullptr &&
        gtk_widget_get_mapped(GTK_WIDGET(interaction_window_))));
    fl_value_set_string_take(interaction, "realized", fl_value_new_bool(
        interaction_window_ != nullptr &&
        gtk_widget_get_realized(GTK_WIDGET(interaction_window_))));
    // GTK draw scheduling is diagnostic data, not a presentation timestamp.
    fl_value_set_string_take(interaction, "gtkDraws",
        fl_value_new_int(interaction_gtk_draws_));
    fl_value_set_string_take(interaction, "firstFrame",
        fl_value_new_bool(interaction_first_frame_));
    fl_value_set_string_take(interaction, "width", fl_value_new_int(
        interaction_window_ == nullptr ? 0 :
            gtk_widget_get_allocated_width(GTK_WIDGET(interaction_window_))));
    fl_value_set_string_take(interaction, "height", fl_value_new_int(
        interaction_window_ == nullptr ? 0 :
            gtk_widget_get_allocated_height(GTK_WIDGET(interaction_window_))));
    fl_value_set_string_take(interaction, "exclusiveZone", fl_value_new_int(-1));
    fl_value_set_string_take(interaction, "cornerPassthrough", fl_value_new_int(0));
    fl_value_set_string_take(state, "interaction", interaction);
    return state;
  }

  FlMethodResponse* AppIcon(FlValue* arguments) const {
    const gchar* icon_name = nullptr;
    const gchar* theme_path = nullptr;
    int size = 48;
    int pixel_size = 0;
    bool valid_pixel_size = true;
    if (arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_STRING) {
      icon_name = fl_value_get_string(arguments);
    } else if (arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP) {
      FlValue* name = fl_value_lookup_string(arguments, "name");
      FlValue* path = fl_value_lookup_string(arguments, "themePath");
      FlValue* requested_size = fl_value_lookup_string(arguments, "size");
      FlValue* pixels = fl_value_lookup_string(arguments, "pixelSize");
      const auto pixel_kind = pixels == nullptr
          ? realand_shell::IconPixelSizeKind::kAbsent
          : fl_value_get_type(pixels) == FL_VALUE_TYPE_INT
              ? realand_shell::IconPixelSizeKind::kInteger
              : realand_shell::IconPixelSizeKind::kInvalid;
      valid_pixel_size = realand_shell::ParseIconPixelSize(
          pixel_kind, pixel_kind == realand_shell::IconPixelSizeKind::kInteger
              ? static_cast<double>(fl_value_get_int(pixels)) : 0, &pixel_size);
      if (name != nullptr && fl_value_get_type(name) == FL_VALUE_TYPE_STRING) {
        icon_name = fl_value_get_string(name);
      }
      if (path != nullptr && fl_value_get_type(path) == FL_VALUE_TYPE_STRING) {
        theme_path = fl_value_get_string(path);
      }
      if (requested_size != nullptr &&
          fl_value_get_type(requested_size) == FL_VALUE_TYPE_INT) {
        size = static_cast<int>(std::max<int64_t>(
            16, std::min<int64_t>(128, fl_value_get_int(requested_size))));
      }
    }
    if (closed_ || window_ == nullptr) {
      return FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The desktop window has closed", nullptr));
    }
    if (!valid_pixel_size || icon_name == nullptr || strlen(icon_name) > 512 ||
        (theme_path != nullptr && strlen(theme_path) > 4096)) {
      return FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "appIcon requires an icon name, bounded theme path, and integer pixelSize from 16 to 512",
          nullptr));
    }
    GtkIconTheme* theme = gtk_icon_theme_get_for_screen(
        gtk_widget_get_screen(GTK_WIDGET(window_)));
    g_autoptr(GtkIconTheme) custom_theme = nullptr;
    if (theme_path != nullptr && *theme_path != '\0') {
      custom_theme = gtk_icon_theme_new();
      gtk_icon_theme_set_screen(custom_theme,
                                gtk_widget_get_screen(GTK_WIDGET(window_)));
      gtk_icon_theme_prepend_search_path(custom_theme, theme_path);
      theme = custom_theme;
    }
    g_autoptr(GtkIconInfo) icon = realand_shell::LookupAppIcon(
        theme, icon_name, size, gtk_widget_get_scale_factor(GTK_WIDGET(window_)),
        pixel_size);
    const gchar* filename = icon == nullptr ? nullptr : gtk_icon_info_get_filename(icon);
    g_autoptr(FlValue) value = filename == nullptr
        ? fl_value_new_null() : fl_value_new_string(filename);
    return FL_METHOD_RESPONSE(fl_method_success_response_new(value));
  }

  static void MethodCall(FlMethodChannel*, FlMethodCall* call, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    const gchar* method = fl_method_call_get_name(call);
    FlValue* arguments = fl_method_call_get_args(call);
    g_autoptr(FlMethodResponse) response = nullptr;
    if (strcmp(method, "appIcon") == 0) {
      response = self->AppIcon(arguments);
    } else if (self->closed_ || self->window_ == nullptr) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The desktop window has closed", nullptr));
    } else if (strcmp(method, "outputName") == 0) {
      g_autoptr(FlValue) name = self->output_name_ == nullptr
          ? fl_value_new_null() : fl_value_new_string(self->output_name_);
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(name));
    } else if (strcmp(method, "windowState") == 0) {
      g_autoptr(FlValue) state = self->WindowState();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(state));
    } else if (strcmp(method, "setShellStyle") == 0) {
      const gchar* style = arguments != nullptr &&
              fl_value_get_type(arguments) == FL_VALUE_TYPE_STRING
          ? fl_value_get_string(arguments) : nullptr;
      if (style == nullptr || (strcmp(style, "macos") != 0 &&
                               strcmp(style, "material") != 0)) {
        response = FL_METHOD_RESPONSE(fl_method_error_response_new(
            "invalid-argument", "setShellStyle requires macos or material", nullptr));
      } else {
        self->macos_shortcuts_ = strcmp(style, "macos") == 0;
        self->PublishShortcutStyle();
        response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
      }
    } else if (strcmp(method, "surfaceOrigin") == 0) {
      const GdkRectangle bounds = self->preview_
          ? GdkRectangle{0, 0, 0, 0} : self->MonitorGeometry();
      g_autoptr(FlValue) origin = fl_value_new_map();
      fl_value_set_string_take(origin, "x", fl_value_new_int(bounds.x));
      fl_value_set_string_take(origin, "y", fl_value_new_int(bounds.y));
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(origin));
    } else if (strcmp(method, "ensureInteractionView") == 0) {
      if (!self->EnsureInteractionView()) {
        response = FL_METHOD_RESPONSE(fl_method_error_response_new(
            "interaction-view-unavailable",
            "Unable to create the desktop interaction view", nullptr));
      } else if (self->preview_) {
        response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
      } else {
        g_autoptr(FlValue) state = fl_value_new_map();
        fl_value_set_string_take(state, "viewId",
            fl_value_new_int(fl_view_get_id(self->interaction_view_)));
        response = FL_METHOD_RESPONSE(fl_method_success_response_new(state));
      }
    } else if (strcmp(method, "setOverlay") == 0) {
      if (arguments == nullptr || fl_value_get_type(arguments) != FL_VALUE_TYPE_BOOL) {
        response = FL_METHOD_RESPONSE(fl_method_error_response_new(
            "invalid-argument", "setOverlay requires a boolean", nullptr));
      } else if (!self->SetOverlay(fl_value_get_bool(arguments))) {
        response = FL_METHOD_RESPONSE(fl_method_error_response_new(
            "interaction-view-unavailable",
            "Unable to open the desktop interaction view", nullptr));
      } else {
        response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
      }
    } else {
      response = FL_METHOD_RESPONSE(fl_method_not_implemented_response_new());
    }
    Respond(call, response);
  }

  static void IconMethodCall(FlMethodChannel*, FlMethodCall* call, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    g_autoptr(FlMethodResponse) response = strcmp(fl_method_call_get_name(call),
                                                "appIcon") == 0
        ? self->AppIcon(fl_method_call_get_args(call))
        : FL_METHOD_RESPONSE(fl_method_not_implemented_response_new());
    Respond(call, response);
  }

  static void FirstFrame(FlView*, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    if (!self->closed_ && self->window_ != nullptr) {
      gtk_widget_show(GTK_WIDGET(self->window_));
    }
  }

  static void Mapped(GtkWidget*, gpointer data) {
    static_cast<DesktopWindow*>(data)->PublishShortcutStyle();
  }

  static void Unrealized(GtkWidget*, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    DestroyMetadata(&self->gtk_surface_);
    self->gtk_metadata_owner_ = nullptr;
  }

  static void SizeAllocated(GtkWidget*, GtkAllocation*, gpointer data) {
    static_cast<DesktopWindow*>(data)->UpdateSurfaceRegions();
  }

  static void StyleUpdated(GtkWidget*, gpointer data) {
    static_cast<DesktopWindow*>(data)->UpdateSurfaceRegions();
  }

  static void WindowDestroyed(GtkWidget*, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    self->closed_ = true;
    self->DestroyInteractionView();
    self->DetachChannels();
  }

  static void InteractionFirstFrame(FlView*, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    if (self->closed_ || self->interaction_window_ == nullptr) return;
    self->interaction_first_frame_ = true;
    if (self->interaction_ready_timeout_ != 0) {
      g_source_remove(self->interaction_ready_timeout_);
      self->interaction_ready_timeout_ = 0;
    }
    self->ConfigureInteractionWindow();
  }

  static gboolean InteractionDrawn(GtkWidget*, cairo_t*, gpointer data) {
    ++static_cast<DesktopWindow*>(data)->interaction_gtk_draws_;
    return FALSE;
  }

  static gboolean InteractionReadyTimeout(gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    self->interaction_ready_timeout_ = 0;
    if (self->closed_ || self->interaction_first_frame_) return G_SOURCE_REMOVE;
    const int64_t view_id = self->interaction_view_ == nullptr
        ? -1 : fl_view_get_id(self->interaction_view_);
    self->overlay_ = false;
    self->DestroyInteractionView();
    self->NotifyInteractionFailed(view_id, "first-frame-timeout");
    return G_SOURCE_REMOVE;
  }

  static void InteractionSizeAllocated(GtkWidget*, GtkAllocation*, gpointer data) {
    static_cast<DesktopWindow*>(data)->UpdateInteractionRegions();
  }

  static void InteractionStyleUpdated(GtkWidget*, gpointer data) {
    static_cast<DesktopWindow*>(data)->UpdateInteractionRegions();
  }

  static gboolean InteractionDeleteEvent(GtkWidget*, GdkEvent*, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    self->overlay_ = false;
    self->ConfigureInteractionWindow();
    self->NotifyInteractionFailed(self->interaction_view_ == nullptr
        ? -1 : fl_view_get_id(self->interaction_view_), "window-closed");
    return TRUE;
  }

  static void InteractionDestroyed(GtkWidget* widget, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    const int64_t view_id = self->interaction_view_ == nullptr
        ? -1 : fl_view_get_id(self->interaction_view_);
    if (self->interaction_ready_timeout_ != 0) {
      g_source_remove(self->interaction_ready_timeout_);
      self->interaction_ready_timeout_ = 0;
    }
    if (self->interaction_view_ != nullptr) {
      g_signal_handlers_disconnect_by_data(self->interaction_view_, self);
      g_object_remove_weak_pointer(G_OBJECT(self->interaction_view_),
          reinterpret_cast<gpointer*>(&self->interaction_view_));
      self->interaction_view_ = nullptr;
    }
    g_signal_handlers_disconnect_by_data(widget, self);
    g_object_remove_weak_pointer(G_OBJECT(widget),
        reinterpret_cast<gpointer*>(&self->interaction_window_));
    self->interaction_window_ = nullptr;
    self->interaction_first_frame_ = false;
    self->overlay_ = false;
    self->NotifyInteractionFailed(view_id, "window-destroyed");
  }

  static void MonitorChanged(GObject*, GParamSpec*, gpointer data) {
    auto* self = static_cast<DesktopWindow*>(data);
    self->ConfigureLayer();
    self->ConfigureInteractionWindow();
    self->UpdateSurfaceRegions();
    self->NotifyOutputChanged();
  }

  static void OutputPosition(void*, wl_proxy*, int32_t, int32_t) {}
  static void OutputSize(void*, wl_proxy*, int32_t, int32_t) {}
  static void OutputDescription(void*, wl_proxy*, const char*) {}
  static void OutputDone(void* data, wl_proxy*) {
    static_cast<DesktopWindow*>(data)->NotifyOutputChanged();
  }
  static void OutputName(void* data, wl_proxy*, const char* name) {
    auto* self = static_cast<DesktopWindow*>(data);
    if (name == nullptr || *name == '\0' || strlen(name) > 512 ||
        !g_utf8_validate(name, -1, nullptr) ||
        g_strcmp0(name, self->output_name_) == 0) return;
    g_free(self->output_name_);
    self->output_name_ = g_strdup(name);
    self->NotifyOutputChanged();
  }

  GdkDisplay* display_ = nullptr;  // Owned by the manager.
  GdkMonitor* monitor_ = nullptr;
  GtkWindow* window_ = nullptr;  // Weak pointers follow GTK ownership.
  FlView* view_ = nullptr;
  GtkWindow* interaction_window_ = nullptr;
  FlView* interaction_view_ = nullptr;
  guint interaction_ready_timeout_ = 0;
  bool interaction_first_frame_ = false;
  int64_t interaction_gtk_draws_ = 0;
  FlMethodChannel* channel_ = nullptr;
  FlMethodChannel* icon_channel_ = nullptr;
  wl_proxy* output_info_ = nullptr;
  wl_proxy* gtk_shell_ = nullptr;  // Owned by the manager, outlives this window.
  wl_proxy* gtk_surface_ = nullptr;
  wl_surface* gtk_metadata_owner_ = nullptr;
  gchar** arguments_ = nullptr;
  gchar* output_name_ = nullptr;
  bool preview_ = false;
  bool macos_shortcuts_ = false;
  bool overlay_ = false;
  bool closed_ = false;
};
}  // namespace

class DesktopWindows::Impl final {
 public:
  Impl(GtkApplication* application, gchar** dart_arguments, bool preview,
       gint preview_monitor, const gchar* preview_output_name)
      : application_(application), arguments_(g_strdupv(dart_arguments)),
        preview_(preview), preview_monitor_(preview_monitor),
        preview_output_name_(g_strdup(preview_output_name)) {}

  ~Impl() {
    if (display_ != nullptr) g_signal_handlers_disconnect_by_data(display_, this);
    windows_.clear();
    DestroyMetadata(&gtk_shell_);
    DestroyProtocol(&output_manager_);
    if (registry_ != nullptr) wl_registry_destroy(registry_);
    g_clear_object(&display_);
    g_clear_pointer(&arguments_, g_strfreev);
    g_clear_pointer(&preview_output_name_, g_free);
    if (held_) g_application_release(G_APPLICATION(application_));
  }

  void Start() {
    if (started_) return;
    started_ = true;
    display_ = GDK_DISPLAY(g_object_ref(gdk_display_get_default()));
    if (preview_) {
      GdkMonitor* monitor = preview_monitor_ >= 0
          ? gdk_display_get_monitor(display_, preview_monitor_)
          : gdk_display_get_primary_monitor(display_);
      if (monitor == nullptr && gdk_display_get_n_monitors(display_) > 0) {
        monitor = gdk_display_get_monitor(display_, 0);
      }
      AddMonitor(monitor);
      return;
    }
    g_application_hold(G_APPLICATION(application_));
    held_ = true;
    g_signal_connect(display_, "monitor-added", G_CALLBACK(MonitorAdded), this);
    g_signal_connect(display_, "monitor-removed", G_CALLBACK(MonitorRemoved), this);
    for (int index = 0; index < gdk_display_get_n_monitors(display_); ++index) {
      AddMonitor(gdk_display_get_monitor(display_, index));
    }
    if (GDK_IS_WAYLAND_DISPLAY(display_)) {
      wl_display* connection = gdk_wayland_display_get_wl_display(display_);
      registry_ = wl_display_get_registry(connection);
      static const wl_registry_listener listener = {RegistryGlobal, RegistryRemoved};
      wl_registry_add_listener(registry_, &listener, this);
      // GDK owns dispatch for this connection. Discovery and names arrive via
      // its normal main loop; no compositor roundtrip blocks engine startup.
      wl_display_flush(connection);
    }
  }

 private:
  void AddMonitor(GdkMonitor* monitor) {
    if (std::any_of(windows_.begin(), windows_.end(),
        [monitor](const std::unique_ptr<DesktopWindow>& window) {
          return window->monitor() == monitor;
        })) return;
    auto window = std::make_unique<DesktopWindow>(application_, display_, monitor,
        arguments_, preview_, preview_output_name_);
    if (output_manager_ != nullptr) window->BindOutputInfo(output_manager_);
    if (gtk_shell_ != nullptr) window->BindGtkMetadata(gtk_shell_);
    windows_.push_back(std::move(window));
  }

  static void MonitorAdded(GdkDisplay*, GdkMonitor* monitor, gpointer data) {
    auto* self = static_cast<Impl*>(data);
    self->AddMonitor(monitor);
    for (const auto& window : self->windows_) window->NotifyOutputChanged();
  }

  static void MonitorRemoved(GdkDisplay*, GdkMonitor* monitor, gpointer data) {
    auto* self = static_cast<Impl*>(data);
    self->windows_.erase(std::remove_if(self->windows_.begin(), self->windows_.end(),
        [monitor](const std::unique_ptr<DesktopWindow>& window) {
          return window->monitor() == monitor;
        }), self->windows_.end());
    for (const auto& window : self->windows_) window->NotifyOutputChanged();
  }

  static void RegistryGlobal(void* data, wl_registry* registry, uint32_t name,
                             const char* interface, uint32_t version) {
    auto* self = static_cast<Impl*>(data);
    if (strcmp(interface, kGtkShellInterface.name) == 0 &&
        version >= 1 && self->gtk_shell_ == nullptr) {
      self->gtk_shell_ = static_cast<wl_proxy*>(
          wl_registry_bind(registry, name, &kGtkShellInterface, 1));
      if (self->gtk_shell_ == nullptr) return;
      static const struct {
        void (*capabilities)(void*, wl_proxy*, uint32_t);
      } listener = {[](void*, wl_proxy*, uint32_t) {}};
      wl_proxy_add_listener(self->gtk_shell_,
          reinterpret_cast<void (**)(void)>(const_cast<void*>(
              static_cast<const void*>(&listener))), self);
      for (const auto& window : self->windows_) {
        window->BindGtkMetadata(self->gtk_shell_);
      }
      return;
    }
    if (strcmp(interface, kXdgManagerInterface.name) != 0 || version < 2 ||
        self->output_manager_ != nullptr) return;
    self->output_manager_ = static_cast<wl_proxy*>(
        wl_registry_bind(registry, name, &kXdgManagerInterface, 2));
    if (self->output_manager_ == nullptr) return;
    self->output_manager_name_ = name;
    for (const auto& window : self->windows_) {
      window->BindOutputInfo(self->output_manager_);
    }
  }

  static void RegistryRemoved(void* data, wl_registry*, uint32_t name) {
    auto* self = static_cast<Impl*>(data);
    if (name == self->output_manager_name_) {
      DestroyProtocol(&self->output_manager_);
      self->output_manager_name_ = 0;
    }
  }

  GtkApplication* application_ = nullptr;
  GdkDisplay* display_ = nullptr;
  gchar** arguments_ = nullptr;
  bool preview_ = false;
  gint preview_monitor_ = -1;
  gchar* preview_output_name_ = nullptr;
  bool started_ = false;
  bool held_ = false;
  std::vector<std::unique_ptr<DesktopWindow>> windows_;
  wl_registry* registry_ = nullptr;
  wl_proxy* output_manager_ = nullptr;
  wl_proxy* gtk_shell_ = nullptr;
  uint32_t output_manager_name_ = 0;
};

DesktopWindows::DesktopWindows(GtkApplication* application, gchar** dart_arguments,
                               bool preview, gint preview_monitor,
                               const gchar* preview_output_name)
    : impl_(std::make_unique<Impl>(application, dart_arguments, preview,
                                   preview_monitor, preview_output_name)) {}

DesktopWindows::~DesktopWindows() = default;

void DesktopWindows::Start() { impl_->Start(); }
