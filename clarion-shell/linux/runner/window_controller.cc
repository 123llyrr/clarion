// SPDX-License-Identifier: GPL-3.0-or-later
#include "window_controller.h"

#include <gtk-layer-shell.h>

#include <algorithm>
#include <cmath>
#include <cstring>

#include "app_icon_lookup.h"
#include "dock_input_region.h"
#include "shell_geometry.h"
#include "startup_trace.h"

namespace {
using clarion_shell::kBarHeight;
using clarion_shell::kDockSurfaceHeight;
constexpr int kCornerPassthrough = 8;
constexpr int kIconSize = 48;

const gchar* SurfaceRole(FlValue* arguments) {
  if (arguments == nullptr || fl_value_get_type(arguments) == FL_VALUE_TYPE_NULL) {
    return "main";
  }
  if (fl_value_get_type(arguments) == FL_VALUE_TYPE_STRING) {
    return fl_value_get_string(arguments);
  }
  if (fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP) {
    FlValue* surface = fl_value_lookup_string(arguments, "surface");
    return surface != nullptr && fl_value_get_type(surface) == FL_VALUE_TYPE_STRING
        ? fl_value_get_string(surface) : nullptr;
  }
  return nullptr;
}

bool ReadFiniteNumber(FlValue* value, double* number) {
  if (value == nullptr) return false;
  double coordinate;
  if (fl_value_get_type(value) == FL_VALUE_TYPE_INT) {
    coordinate = static_cast<double>(fl_value_get_int(value));
  } else if (fl_value_get_type(value) == FL_VALUE_TYPE_FLOAT) {
    coordinate = fl_value_get_float(value);
  } else {
    return false;
  }
  if (!std::isfinite(coordinate) || std::abs(coordinate) > 1000000) return false;
  *number = coordinate;
  return true;
}

bool ReadCoordinate(FlValue* value, int* number) {
  double coordinate;
  if (!ReadFiniteNumber(value, &coordinate)) return false;
  *number = static_cast<int>(std::ceil(coordinate));
  return true;
}

bool ReadDockInputRect(FlValue* value, clarion_shell::InputRect* rect) {
  double x, y, width, height;
  return value != nullptr && fl_value_get_type(value) == FL_VALUE_TYPE_MAP &&
      ReadFiniteNumber(fl_value_lookup_string(value, "x"), &x) &&
      ReadFiniteNumber(fl_value_lookup_string(value, "y"), &y) &&
      ReadFiniteNumber(fl_value_lookup_string(value, "width"), &width) &&
      ReadFiniteNumber(fl_value_lookup_string(value, "height"), &height) &&
      clarion_shell::EncloseDockInputRect(x, y, width, height, rect);
}

void Respond(FlMethodCall* call, FlMethodResponse* response) {
  g_autoptr(GError) error = nullptr;
  if (!fl_method_call_respond(call, response, &error)) {
    g_warning("Unable to respond to Clarion Shell method: %s", error->message);
  }
}
}  // namespace

WindowController::WindowController(GtkWindow* window, FlView* view, bool preview,
                                   GdkMonitor* monitor,
                                   const gchar* output_name)
    : window_(window), view_(view), output_name_(g_strdup(output_name)),
      preview_(preview) {
  g_object_add_weak_pointer(G_OBJECT(window_),
                           reinterpret_cast<gpointer*>(&window_));
  g_object_add_weak_pointer(G_OBJECT(view_),
                           reinterpret_cast<gpointer*>(&view_));
  display_ = GDK_DISPLAY(g_object_ref(gtk_widget_get_display(GTK_WIDGET(window))));
  size_handler_ = g_signal_connect(window_, "size-allocate",
                                   G_CALLBACK(SizeAllocated), this);
  style_handler_ = g_signal_connect(window_, "style-updated",
                                    G_CALLBACK(StyleUpdated), this);
  destroy_handler_ = g_signal_connect(window_, "destroy",
                                      G_CALLBACK(WindowDestroyed), this);

  if (!preview_) {
    main_effect_ = std::make_unique<BackgroundEffect>(GTK_WIDGET(window_));
    gtk_layer_init_for_window(window_);
    gtk_layer_set_namespace(window_, "clarion-shell");
    removed_handler_ = g_signal_connect(display_, "monitor-removed",
                                        G_CALLBACK(MonitorRemoved), this);
    SetMonitor(monitor);
    ConfigureLayerWindow();
  } else {
    SetMonitor(monitor);
  }

  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  channel_ = fl_method_channel_new(
      fl_engine_get_binary_messenger(fl_view_get_engine(view_)),
      "clarion.shell/window", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(channel_, MethodCall, this, nullptr);
}

WindowController::~WindowController() {
  // Remove the callback first: neither the engine nor a pending channel message
  // can subsequently access the controller while GTK tears down its windows.
  if (channel_ != nullptr) {
    fl_method_channel_set_method_call_handler(channel_, nullptr, nullptr, nullptr);
    g_clear_object(&channel_);
  }
  DestroyDock();
  DestroyReservations();
  panel_glass_region_.reset();
  main_effect_.reset();
  if (monitor_ != nullptr && geometry_handler_ != 0) {
    g_signal_handler_disconnect(monitor_, geometry_handler_);
  }
  if (monitor_ != nullptr && monitor_scale_handler_ != 0) {
    g_signal_handler_disconnect(monitor_, monitor_scale_handler_);
  }
  g_clear_object(&monitor_);
  if (display_ != nullptr && removed_handler_ != 0) {
    g_signal_handler_disconnect(display_, removed_handler_);
  }
  g_clear_object(&display_);
  if (window_ != nullptr) {
    g_signal_handler_disconnect(window_, size_handler_);
    g_signal_handler_disconnect(window_, style_handler_);
    g_signal_handler_disconnect(window_, destroy_handler_);
    g_object_remove_weak_pointer(G_OBJECT(window_),
                                reinterpret_cast<gpointer*>(&window_));
  }
  if (view_ != nullptr) {
    g_object_remove_weak_pointer(G_OBJECT(view_),
                                reinterpret_cast<gpointer*>(&view_));
  }
  g_free(output_name_);
}

void WindowController::SetMonitor(GdkMonitor* monitor) {
  if (monitor_ == monitor) return;
  if (monitor_ != nullptr && geometry_handler_ != 0) {
    g_signal_handler_disconnect(monitor_, geometry_handler_);
  }
  if (monitor_ != nullptr && monitor_scale_handler_ != 0) {
    g_signal_handler_disconnect(monitor_, monitor_scale_handler_);
  }
  geometry_handler_ = 0;
  monitor_scale_handler_ = 0;
  g_set_object(&monitor_, monitor);
  if (monitor_ != nullptr) {
    geometry_handler_ = g_signal_connect(
        monitor_, "notify::geometry", G_CALLBACK(MonitorGeometryChanged), this);
    monitor_scale_handler_ = g_signal_connect(
        monitor_, "notify::scale-factor", G_CALLBACK(MonitorGeometryChanged), this);
  }
  if (window_ != nullptr && !preview_) gtk_layer_set_monitor(window_, monitor_);
  if (dock_window_ != nullptr && !preview_) {
    gtk_layer_set_monitor(dock_window_, monitor_);
  }
  for (GtkWindow* reservation : {menu_reservation_, dock_reservation_}) {
    if (reservation != nullptr) gtk_layer_set_monitor(reservation, monitor_);
  }
}

int WindowController::BarHeight() const {
  return clarion_shell::BarHeight(macos_, mac_menu_height_);
}

bool WindowController::BarBottom() const {
  return bar_bottom_ && !macos_;
}

bool WindowController::MainKeyboardExclusive() const {
  return expanded_ && keyboard_interactivity_override_.value_or(true);
}

void WindowController::ConfigureLayerWindow() {
  if (window_ == nullptr || preview_) return;
  const bool centered_menu = macos_ && !expanded_ && mac_menu_width_percent_ < 100;
  // The selected vertical edge positions the strip and its expanded overlay.
  // Material also reserves that edge. macOS reserves it independently below.
  gtk_layer_set_anchor(window_, GTK_LAYER_SHELL_EDGE_TOP, !BarBottom());
  gtk_layer_set_anchor(window_, GTK_LAYER_SHELL_EDGE_LEFT, !centered_menu);
  gtk_layer_set_anchor(window_, GTK_LAYER_SHELL_EDGE_RIGHT, !centered_menu);
  gtk_layer_set_anchor(window_, GTK_LAYER_SHELL_EDGE_BOTTOM, BarBottom());
  gtk_layer_set_layer(window_, expanded_ ? GTK_LAYER_SHELL_LAYER_OVERLAY
                                       : GTK_LAYER_SHELL_LAYER_TOP);
  // Drawing surfaces ignore other layer reservations. Separate invisible,
  // input-free surfaces reserve the desktop edges without moving the menu,
  // vertical Dock or full-output modal overlay into a reduced working area.
  gtk_layer_set_exclusive_zone(window_, macos_ ? -1 : BarHeight());
  gtk_layer_set_keyboard_mode(window_, MainKeyboardExclusive()
      ? GTK_LAYER_SHELL_KEYBOARD_MODE_EXCLUSIVE
      : GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);

  // The surface is always full-output sized. Opening a panel therefore never
  // resizes the window, so the Flutter framebuffer always matches the surface
  // size and the compositor can never time out and blit a stale strip frame
  // stretched across the output. When collapsed, only the bar strip is opaque
  // and input-receptive; the rest is transparent and passes clicks through
  // (UpdateInputRegion). This removes the resize-induced flash entirely.
  const GdkRectangle geometry = MonitorGeometry();
  const int height = std::max(BarHeight(), geometry.height);
  const int width = centered_menu ? std::max(1, static_cast<int>(std::round(
      MonitorGeometry().width * mac_menu_width_percent_ / 100))) : -1;
  gtk_widget_set_size_request(GTK_WIDGET(window_), width, height);
  gtk_window_resize(window_, width > 0 ? width : 1, 1);
  ConfigureReservations();
  UpdateBackgroundEffects();
}

bool WindowController::EnsureReservationWindow(GtkWindow** slot, const gchar* name) {
  if (*slot != nullptr) return true;
  if (window_ == nullptr || preview_ || closed_) return false;
  GtkApplication* app = gtk_window_get_application(window_);
  if (app == nullptr) return false;
  GtkWindow* reservation = GTK_WINDOW(gtk_application_window_new(app));
  gtk_application_window_set_show_menubar(
      GTK_APPLICATION_WINDOW(reservation), FALSE);
  gtk_window_set_decorated(reservation, FALSE);
  gtk_window_set_resizable(reservation, FALSE);
  gtk_window_set_accept_focus(reservation, FALSE);
  gtk_window_set_skip_taskbar_hint(reservation, TRUE);
  gtk_window_set_skip_pager_hint(reservation, TRUE);
  gtk_widget_set_app_paintable(GTK_WIDGET(reservation), TRUE);
  GdkVisual* visual = gdk_screen_get_rgba_visual(gtk_widget_get_screen(GTK_WIDGET(reservation)));
  if (visual != nullptr) gtk_widget_set_visual(GTK_WIDGET(reservation), visual);
  gtk_layer_init_for_window(reservation);
  gtk_layer_set_namespace(reservation, name);
  gtk_layer_set_monitor(reservation, monitor_);
  gtk_layer_set_layer(reservation, GTK_LAYER_SHELL_LAYER_TOP);
  gtk_layer_set_keyboard_mode(reservation, GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
  gtk_widget_set_size_request(GTK_WIDGET(reservation), 1, 1);
  g_signal_connect(reservation, "realize", G_CALLBACK(ReservationInput), nullptr);
  g_signal_connect(reservation, "size-allocate", G_CALLBACK(ReservationSizeAllocated), nullptr);
  g_signal_connect(reservation, "draw", G_CALLBACK(ReservationDraw), nullptr);
  g_signal_connect(reservation, "delete-event", G_CALLBACK(DockDeleteEvent), nullptr);
  *slot = reservation;
  g_object_add_weak_pointer(G_OBJECT(reservation), reinterpret_cast<gpointer*>(slot));
  return true;
}

void WindowController::ReservationInput(GtkWidget* widget, gpointer) {
  GdkWindow* window = gtk_widget_get_window(widget);
  if (window == nullptr) return;
  gdk_window_set_opaque_region(window, nullptr);
  cairo_region_t* empty = cairo_region_create();
  gdk_window_input_shape_combine_region(window, empty, 0, 0);
  cairo_region_destroy(empty);
}

void WindowController::ReservationSizeAllocated(GtkWidget* widget, GtkAllocation*, gpointer) {
  ReservationInput(widget, nullptr);
}

gboolean WindowController::ReservationDraw(GtkWidget*, cairo_t* cairo, gpointer) {
  cairo_set_operator(cairo, CAIRO_OPERATOR_SOURCE);
  cairo_set_source_rgba(cairo, 0, 0, 0, 0);
  cairo_paint(cairo);
  return TRUE;
}

void WindowController::ConfigureReservations() {
  if (preview_ || closed_) return;
  if (macos_) {
    EnsureReservationWindow(&menu_reservation_, "clarion-shell-menu-reservation");
    EnsureReservationWindow(&dock_reservation_, "clarion-shell-dock-reservation");
  }
  for (GtkWindow* reservation : {menu_reservation_, dock_reservation_}) {
    if (reservation == nullptr) continue;
    const bool menu = reservation == menu_reservation_;
    const int zone = !macos_ ? 0 : menu ? (mac_menu_auto_hide_ ? 0 : BarHeight())
        : (dock_auto_hide_ || !dock_first_frame_ ? 0 : dock_reserved_size_);
    gtk_layer_set_anchor(reservation, GTK_LAYER_SHELL_EDGE_TOP, menu);
    gtk_layer_set_anchor(reservation, GTK_LAYER_SHELL_EDGE_BOTTOM,
        !menu && dock_position_ == clarion_shell::DockPosition::Bottom);
    gtk_layer_set_anchor(reservation, GTK_LAYER_SHELL_EDGE_LEFT,
        !menu && dock_position_ == clarion_shell::DockPosition::Left);
    gtk_layer_set_anchor(reservation, GTK_LAYER_SHELL_EDGE_RIGHT,
        !menu && dock_position_ == clarion_shell::DockPosition::Right);
    gtk_layer_set_exclusive_zone(reservation, zone);
    if (zone > 0) gtk_widget_show(GTK_WIDGET(reservation));
    else gtk_widget_hide(GTK_WIDGET(reservation));
  }
}

void WindowController::DestroyReservations() {
  for (GtkWindow** slot : {&menu_reservation_, &dock_reservation_}) {
    if (*slot == nullptr) continue;
    GtkWindow* reservation = *slot;
    g_object_remove_weak_pointer(G_OBJECT(reservation), reinterpret_cast<gpointer*>(slot));
    *slot = nullptr;
    gtk_widget_destroy(GTK_WIDGET(reservation));
  }
}

bool WindowController::SetExpanded(bool expanded) {
  if (window_ == nullptr || closed_) return false;
  if (!expanded) keyboard_interactivity_override_.reset();
  if (expanded_ == expanded) return true;
  expanded_ = expanded;
  if (!expanded_) {
    panel_glass_region_.reset();
    launcher_backdrop_ = false;
    launcher_dock_interactive_ = false;
  }
  if (expanded_) {
    tooltip_visible_ = false;
    dock_tooltip_visible_ = false;
  }
  ConfigureLayerWindow();
  ConfigureDockWindow();
  UpdateInputRegion();
  UpdateBackgroundEffects();
  if (MainKeyboardExclusive() && view_ != nullptr) {
    gtk_widget_grab_focus(GTK_WIDGET(view_));
  }
  return true;
}

bool WindowController::SetKeyboardInteractivity(std::optional<bool> exclusive) {
  if (window_ == nullptr || closed_) return false;
  keyboard_interactivity_override_ = exclusive;
  ConfigureLayerWindow();
  if (MainKeyboardExclusive() && view_ != nullptr) {
    gtk_widget_grab_focus(GTK_WIDGET(view_));
  }
  return true;
}

bool WindowController::SetTooltipVisible(bool visible, bool dock) {
  if (window_ == nullptr || closed_) return false;
  // A delayed hover reply cannot shrink a menu or change its keyboard grab.
  visible = visible && !expanded_ && !preview_;
  if (dock) {
    if (dock_tooltip_visible_ == visible) return true;
    dock_tooltip_visible_ = visible && macos_;
    ConfigureDockWindow();
    return true;
  }
  if (tooltip_visible_ == visible) return true;
  tooltip_visible_ = visible;
  ConfigureLayerWindow();
  UpdateInputRegion();
  return true;
}

bool WindowController::SetBarPosition(bool bottom) {
  if (window_ == nullptr || closed_) return false;
  if (bar_bottom_ == bottom) return true;
  DismissTooltip();
  bar_bottom_ = bottom;
  ConfigureLayerWindow();
  ConfigureDockWindow();
  UpdateInputRegion();
  return true;
}

bool WindowController::EnsureDock() {
  if (dock_window_ != nullptr && dock_view_ != nullptr) return true;
  if (window_ == nullptr || view_ == nullptr || closed_) return false;
  GtkApplication* application = gtk_window_get_application(window_);
  if (application == nullptr) return false;
  GtkWindow* window = GTK_WINDOW(gtk_application_window_new(application));
  // An appmenu-injected native menubar must not draw above the transparent Dock.
  gtk_application_window_set_show_menubar(GTK_APPLICATION_WINDOW(window), FALSE);
  gtk_window_set_title(window, "Clarion Dock");
  gtk_window_set_decorated(window, FALSE);
  gtk_window_set_resizable(window, FALSE);
  gtk_window_set_accept_focus(window, FALSE);
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
  if (!preview_) {
    gtk_layer_init_for_window(window);
    gtk_layer_set_namespace(window, "clarion-shell-dock");
    gtk_layer_set_monitor(window, monitor_);
  }

  // This adds a view to the existing engine. Registering another method
  // channel or plugins here would replace the main view's shared handlers.
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
  dock_window_ = window;
  dock_view_ = view;
  if (!preview_) dock_effect_ = std::make_unique<BackgroundEffect>(GTK_WIDGET(window));
  g_object_add_weak_pointer(G_OBJECT(dock_window_),
                           reinterpret_cast<gpointer*>(&dock_window_));
  g_object_add_weak_pointer(G_OBJECT(dock_view_),
                           reinterpret_cast<gpointer*>(&dock_view_));
  dock_size_handler_ = g_signal_connect(window, "size-allocate",
      G_CALLBACK(DockSizeAllocated), this);
  dock_style_handler_ = g_signal_connect(window, "style-updated",
      G_CALLBACK(DockStyleUpdated), this);
  dock_delete_handler_ = g_signal_connect(window, "delete-event",
      G_CALLBACK(DockDeleteEvent), this);
  dock_first_frame_handler_ = g_signal_connect(view, "first-frame",
      G_CALLBACK(DockFirstFrame), this);
  ConfigureDockWindow();
  gtk_widget_show(GTK_WIDGET(view));
  gtk_widget_realize(GTK_WIDGET(view));
  return true;
}

void WindowController::DestroyDock() {
  dock_effect_.reset();
  if (dock_ready_timeout_ != 0) {
    g_source_remove(dock_ready_timeout_);
    dock_ready_timeout_ = 0;
  }
  if (dock_view_ != nullptr) {
    g_signal_handler_disconnect(dock_view_, dock_first_frame_handler_);
    g_object_remove_weak_pointer(G_OBJECT(dock_view_),
                                reinterpret_cast<gpointer*>(&dock_view_));
    dock_view_ = nullptr;
  }
  if (dock_window_ != nullptr) {
    GtkWindow* window = dock_window_;
    g_signal_handler_disconnect(window, dock_size_handler_);
    g_signal_handler_disconnect(window, dock_style_handler_);
    g_signal_handler_disconnect(window, dock_delete_handler_);
    g_object_remove_weak_pointer(G_OBJECT(window),
                                reinterpret_cast<gpointer*>(&dock_window_));
    dock_window_ = nullptr;
    gtk_widget_destroy(GTK_WIDGET(window));
  }
  dock_first_frame_ = false;
  dock_tooltip_visible_ = false;
}

bool WindowController::SetShellStyle(bool macos) {
  if (window_ == nullptr || closed_) return false;
  if (macos && !EnsureDock()) return false;
  if (macos_ == macos) return true;
  DismissTooltip();
  expanded_ = false;
  keyboard_interactivity_override_.reset();
  panel_glass_region_.reset();
  macos_ = macos;
  if (!macos_) {
    launcher_backdrop_ = false;
    launcher_dock_interactive_ = false;
  }
  if (dock_ready_timeout_ != 0) {
    g_source_remove(dock_ready_timeout_);
    dock_ready_timeout_ = 0;
  }
  if (macos_ && !dock_first_frame_) {
    // Return the new view ID immediately so Dart can attach a View widget.
    // A failed asynchronous AddView must not leave the shell in a style
    // whose application controls never appear.
    dock_ready_timeout_ = g_timeout_add_seconds(10, DockReadyTimeout, this);
  }
  ConfigureLayerWindow();
  UpdateInputRegion();
  ConfigureDockWindow();
  return true;
}

void WindowController::ConfigureDockWindow() {
  if (dock_window_ == nullptr) return;
  const GdkRectangle geometry = MonitorGeometry();
  const auto placement = clarion_shell::PlaceDock(
      dock_position_, dock_desired_width_, dock_desired_height_,
      geometry.width, geometry.height);
  const int width = placement.width;
  const int height = placement.height;
  if (!preview_) {
    gtk_layer_set_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_TOP, FALSE);
    gtk_layer_set_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_LEFT,
        dock_position_ == clarion_shell::DockPosition::Left);
    gtk_layer_set_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_RIGHT,
        dock_position_ == clarion_shell::DockPosition::Right);
    gtk_layer_set_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_BOTTOM,
        dock_position_ == clarion_shell::DockPosition::Bottom);
    gtk_layer_set_layer(dock_window_, macos_ && launcher_backdrop_
        ? GTK_LAYER_SHELL_LAYER_OVERLAY : GTK_LAYER_SHELL_LAYER_TOP);
    gtk_layer_set_keyboard_mode(dock_window_, GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
    gtk_layer_set_exclusive_zone(dock_window_, -1);
  }
  gtk_widget_set_size_request(GTK_WIDGET(dock_window_), width, height);
  gtk_window_resize(dock_window_, width, height);
  if (preview_) {
    gtk_window_move(dock_window_, geometry.x + placement.x, geometry.y + placement.y);
  }
  if (macos_ && dock_first_frame_) {
    gtk_widget_show(GTK_WIDGET(dock_window_));
  } else {
    gtk_widget_hide(GTK_WIDGET(dock_window_));
  }
  ClearDockOpaqueRegion();
  ConfigureReservations();
  UpdateDockInputRegion();
  UpdateBackgroundEffects();
}

void WindowController::UpdateBackgroundEffects() {
  if (main_effect_ != nullptr && window_ != nullptr) {
    const int surface_width = gtk_widget_get_allocated_width(GTK_WIDGET(window_));
    const bool launcher = macos_ && expanded_ && launcher_backdrop_;
    const int menu_width = std::min(surface_width, static_cast<int>(std::round(
        MonitorGeometry().width * mac_menu_width_percent_ / 100.0)));
    std::vector<clarion_shell::GlassRegion> regions;
    // One stable native mask shares the compositor's cached liquid-glass
    // backdrop across icon/search animation frames and the retained exit.
    // Collapsing or changing style falls back to the ordinary bar/panel mask.
    if (launcher && !closed_) {
      const int surface_height = gtk_widget_get_allocated_height(GTK_WIDGET(window_));
      regions.push_back({0, 0, static_cast<double>(surface_width),
          static_cast<double>(surface_height), 0});
    } else if (macos_ && !closed_) {
      if (!mac_menu_auto_hide_ || mac_menu_visible_ || expanded_) {
        regions.push_back({static_cast<double>((surface_width - menu_width) / 2),
            0, static_cast<double>(menu_width), static_cast<double>(BarHeight()), 0});
      }
      if (expanded_ && panel_glass_region_) regions.push_back(*panel_glass_region_);
    }
    main_effect_->SetRegions(regions);
  }
  if (dock_effect_ != nullptr) {
    dock_effect_->SetRegion(macos_ && dock_first_frame_ && !closed_ &&
        dock_glass_width_ > 0 && dock_glass_height_ > 0,
        dock_glass_x_, dock_glass_y_, dock_glass_width_, dock_glass_height_, dock_glass_radius_);
  }
}

void WindowController::ClearDockOpaqueRegion() {
  if (dock_window_ == nullptr) return;
  GdkWindow* window = gtk_widget_get_window(GTK_WIDGET(dock_window_));
  if (window != nullptr) gdk_window_set_opaque_region(window, nullptr);
}

void WindowController::UpdateDockInputRegion() {
  dock_applied_input_regions_.clear();
  if (dock_window_ == nullptr) return;
  GdkWindow* window = gtk_widget_get_window(GTK_WIDGET(dock_window_));
  if (window == nullptr) return;
  cairo_region_t* region = cairo_region_create();
  if (macos_ && dock_first_frame_ && (!expanded_ || launcher_dock_interactive_)) {
    const int width = gtk_widget_get_allocated_width(GTK_WIDGET(dock_window_));
    const int height = gtk_widget_get_allocated_height(GTK_WIDGET(dock_window_));
    const clarion_shell::InputRect requested = dock_custom_input_ ? dock_input_
        : clarion_shell::InputRect{0, 20, width, kDockSurfaceHeight - 20};
    cairo_region_destroy(region);
    region = dock_input_regions_
        ? clarion_shell::BuildDockInputRegion(*dock_input_regions_, width, height)
        : clarion_shell::BuildDockInputRegion({requested}, width, height, true);
    for (int index = 0; index < cairo_region_num_rectangles(region); ++index) {
      cairo_rectangle_int_t rect;
      cairo_region_get_rectangle(region, index, &rect);
      dock_applied_input_regions_.push_back({rect.x, rect.y, rect.width, rect.height});
    }
  }
  gdk_window_input_shape_combine_region(window, region, 0, 0);
  cairo_region_destroy(region);
}

GdkRectangle WindowController::MonitorGeometry() const {
  GdkRectangle geometry = {0, 0, 0, 0};
  GdkMonitor* monitor = monitor_;
  if (monitor == nullptr && window_ != nullptr && !preview_) {
    GdkWindow* window = gtk_widget_get_window(GTK_WIDGET(window_));
    if (window != nullptr) {
      monitor = gdk_display_get_monitor_at_window(display_, window);
    }
  }
  if (monitor != nullptr) gdk_monitor_get_geometry(monitor, &geometry);
  return geometry;
}

int WindowController::InputY(int height) const {
  return clarion_shell::BarInputY(height, bar_bottom_, expanded_, preview_, macos_);
}

void WindowController::DismissTooltip() {
  if (!tooltip_visible_ && !dock_tooltip_visible_) return;
  tooltip_visible_ = false;
  dock_tooltip_visible_ = false;
  // Output relocation invalidates Flutter's stored icon anchor as well.
  if (channel_ != nullptr) {
    fl_method_channel_invoke_method(channel_, "dismissBarTooltip", nullptr,
                                   nullptr, nullptr, nullptr);
  }
}

void WindowController::ClearOpaqueRegion() {
  if (window_ == nullptr) return;
  GdkWindow* window = gtk_widget_get_window(GTK_WIDGET(window_));
  if (window != nullptr) gdk_window_set_opaque_region(window, nullptr);
}

void WindowController::UpdateInputRegion() {
  if (window_ == nullptr) return;
  GdkWindow* window = gtk_widget_get_window(GTK_WIDGET(window_));
  if (window == nullptr) return;
  if (preview_ || expanded_) {
    gdk_window_input_shape_combine_region(window, nullptr, 0, 0);
    return;
  }
  GtkAllocation allocation;
  gtk_widget_get_allocation(GTK_WIDGET(window_), &allocation);
  const int input_y = InputY(allocation.height);
  const cairo_rectangle_int_t full = {
      0, input_y, allocation.width,
      clarion_shell::BarInputHeight(macos_, mac_menu_auto_hide_, mac_menu_visible_, allocation.height, mac_menu_height_)};
  cairo_region_t* region = cairo_region_create_rectangle(&full);
  const int corner = std::min(kCornerPassthrough, allocation.width / 2);
  const int corner_y = BarBottom()
      ? std::max(input_y, allocation.height - kCornerPassthrough) : input_y;
  const cairo_rectangle_int_t left = {0, corner_y, corner, kCornerPassthrough};
  const cairo_rectangle_int_t right = {
      allocation.width - corner, corner_y, corner, kCornerPassthrough};
  cairo_region_subtract_rectangle(region, &left);
  cairo_region_subtract_rectangle(region, &right);
  gdk_window_input_shape_combine_region(window, region, 0, 0);
  cairo_region_destroy(region);
}

void WindowController::SizeAllocated(GtkWidget*, GtkAllocation*, gpointer data) {
  auto* self = static_cast<WindowController*>(data);
  self->ClearOpaqueRegion();
  self->UpdateInputRegion();
  self->UpdateBackgroundEffects();
}

void WindowController::StyleUpdated(GtkWidget*, gpointer data) {
  static_cast<WindowController*>(data)->ClearOpaqueRegion();
}

void WindowController::WindowDestroyed(GtkWidget*, gpointer data) {
  auto* self = static_cast<WindowController*>(data);
  self->closed_ = true;
  self->panel_glass_region_.reset();
  self->main_effect_.reset();
  self->DestroyDock();
  self->DestroyReservations();
}

void WindowController::DockSizeAllocated(GtkWidget*, GtkAllocation*, gpointer data) {
  auto* self = static_cast<WindowController*>(data);
  self->ClearDockOpaqueRegion();
  self->UpdateDockInputRegion();
  self->UpdateBackgroundEffects();
}

void WindowController::DockStyleUpdated(GtkWidget*, gpointer data) {
  static_cast<WindowController*>(data)->ClearDockOpaqueRegion();
}

void WindowController::DockFirstFrame(FlView*, gpointer data) {
  clarion_shell::TraceStartup("dock-first-frame");
  auto* self = static_cast<WindowController*>(data);
  self->dock_first_frame_ = true;
  if (self->dock_ready_timeout_ != 0) {
    g_source_remove(self->dock_ready_timeout_);
    self->dock_ready_timeout_ = 0;
  }
  self->ConfigureDockWindow();
}

gboolean WindowController::DockReadyTimeout(gpointer data) {
  auto* self = static_cast<WindowController*>(data);
  self->dock_ready_timeout_ = 0;
  if (self->closed_ || !self->macos_ || self->dock_first_frame_) return G_SOURCE_REMOVE;
  self->macos_ = false;
  self->expanded_ = false;
  self->keyboard_interactivity_override_.reset();
  self->panel_glass_region_.reset();
  self->DismissTooltip();
  self->DestroyDock();
  self->ConfigureLayerWindow();
  self->UpdateInputRegion();
  g_autoptr(FlValue) state = self->WindowState();
  fl_value_set_string_take(state, "reason", fl_value_new_string("dock-first-frame-timeout"));
  fl_method_channel_invoke_method(self->channel_, "shellStyleFallback", state,
                                 nullptr, nullptr, nullptr);
  g_warning("Dock did not render its first frame; restored Material shell");
  return G_SOURCE_REMOVE;
}

gboolean WindowController::DockDeleteEvent(GtkWidget*, GdkEvent*, gpointer) {
  // The Dock belongs to the main window, including in preview mode.
  return TRUE;
}

void WindowController::PublishOutputGeometry() {
  if (channel_ == nullptr || window_ == nullptr || closed_) return;
  // Height/scale changes can relocate a bottom-anchored Dock without changing
  // either Flutter view's own dimensions. Force a fresh output-local origin.
  fl_method_channel_invoke_method(channel_, "outputGeometryChanged", nullptr,
                                 nullptr, nullptr, nullptr);
}

void WindowController::MonitorGeometryChanged(GObject*, GParamSpec*, gpointer data) {
  auto* self = static_cast<WindowController*>(data);
  self->DismissTooltip();
  self->ConfigureLayerWindow();
  self->ConfigureDockWindow();
  self->UpdateInputRegion();
  self->PublishOutputGeometry();
}

void WindowController::MonitorRemoved(GdkDisplay* display, GdkMonitor* monitor,
                                      gpointer data) {
  auto* self = static_cast<WindowController*>(data);
  if (self->monitor_ != monitor) return;
  self->DismissTooltip();
  GdkMonitor* replacement = gdk_display_get_primary_monitor(display);
  if (replacement == monitor) replacement = nullptr;
  if (replacement == nullptr) {
    for (int i = 0; i < gdk_display_get_n_monitors(display); ++i) {
      GdkMonitor* candidate = gdk_display_get_monitor(display, i);
      if (candidate != monitor) { replacement = candidate; break; }
    }
  }
  self->SetMonitor(replacement);
  // An explicit output name no longer identifies this surface after hot unplug.
  g_clear_pointer(&self->output_name_, g_free);
  self->ConfigureLayerWindow();
  self->ConfigureDockWindow();
  self->UpdateInputRegion();
  self->PublishOutputGeometry();
}

FlValue* WindowController::WindowState() const {
  g_autoptr(FlValue) state = fl_value_new_map();
  fl_value_set_string_take(state, "style", fl_value_new_string(macos_ ? "macos" : "material"));
  fl_value_set_string_take(state, "shellStyle", fl_value_new_string(macos_ ? "macos" : "material"));
  fl_value_set_string_take(state, "mainViewId", view_ == nullptr
      ? fl_value_new_null() : fl_value_new_int(fl_view_get_id(view_)));
  fl_value_set_string_take(state, "dockViewId", dock_view_ == nullptr
      ? fl_value_new_null() : fl_value_new_int(fl_view_get_id(dock_view_)));
  fl_value_set_string_take(state, "dockVisible", fl_value_new_bool(
      dock_window_ != nullptr && gtk_widget_get_visible(GTK_WIDGET(dock_window_))));
  fl_value_set_string_take(state, "dockTooltipVisible", fl_value_new_bool(dock_tooltip_visible_));
  if (dock_window_ != nullptr) {
    g_autoptr(FlValue) dock = fl_value_new_map();
    const int width = gtk_widget_get_allocated_width(GTK_WIDGET(dock_window_));
    const int height = gtk_widget_get_allocated_height(GTK_WIDGET(dock_window_));
    const clarion_shell::InputRect requested = dock_custom_input_ ? dock_input_
        : clarion_shell::InputRect{0, 20, width, kDockSurfaceHeight - 20};
    const clarion_shell::InputRect rect = clarion_shell::DockInputRect(requested, width, height);
    const bool input_enabled = macos_ && dock_first_frame_ && (!expanded_ || launcher_dock_interactive_);
    fl_value_set_string_take(dock, "width", fl_value_new_int(width));
    fl_value_set_string_take(dock, "height", fl_value_new_int(height));
    fl_value_set_string_take(dock, "visible", fl_value_new_bool(
        gtk_widget_get_visible(GTK_WIDGET(dock_window_))));
    fl_value_set_string_take(dock, "ready", fl_value_new_bool(dock_first_frame_));
    fl_value_set_string_take(dock, "inputX", fl_value_new_int(rect.x));
    fl_value_set_string_take(dock, "inputY", fl_value_new_int(rect.y));
    fl_value_set_string_take(dock, "inputWidth", fl_value_new_int(input_enabled ? rect.width : 0));
    fl_value_set_string_take(dock, "inputHeight", fl_value_new_int(input_enabled ? rect.height : 0));
    g_autoptr(FlValue) input_regions = fl_value_new_list();
    for (const auto& applied : dock_applied_input_regions_) {
      g_autoptr(FlValue) region = fl_value_new_map();
      fl_value_set_string_take(region, "x", fl_value_new_int(applied.x));
      fl_value_set_string_take(region, "y", fl_value_new_int(applied.y));
      fl_value_set_string_take(region, "width", fl_value_new_int(applied.width));
      fl_value_set_string_take(region, "height", fl_value_new_int(applied.height));
      fl_value_append_take(input_regions, g_steal_pointer(&region));
    }
    fl_value_set_string_take(dock, "inputRegions", g_steal_pointer(&input_regions));
    fl_value_set_string_take(dock, "layer", fl_value_new_string(preview_ ? "Preview"
        : gtk_layer_get_layer(dock_window_) == GTK_LAYER_SHELL_LAYER_OVERLAY ? "Overlay" : "Top"));
    fl_value_set_string_take(dock, "keyboard", fl_value_new_string("None"));
    fl_value_set_string_take(dock, "position", fl_value_new_string(
        dock_position_ == clarion_shell::DockPosition::Left ? "left" :
        dock_position_ == clarion_shell::DockPosition::Right ? "right" : "bottom"));
    fl_value_set_string_take(dock, "autoHide", fl_value_new_bool(dock_auto_hide_));
    fl_value_set_string_take(dock, "exclusiveZone", fl_value_new_int(
        preview_ ? 0 : gtk_layer_get_exclusive_zone(dock_window_)));
    fl_value_set_string_take(dock, "reservationZone", fl_value_new_int(
        dock_reservation_ != nullptr && gtk_widget_get_visible(GTK_WIDGET(dock_reservation_))
        ? gtk_layer_get_exclusive_zone(dock_reservation_) : 0));
    fl_value_set_string_take(dock, "anchorTop", fl_value_new_bool(!preview_ &&
        gtk_layer_get_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_TOP)));
    fl_value_set_string_take(dock, "anchorBottom", fl_value_new_bool(!preview_ &&
        gtk_layer_get_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_BOTTOM)));
    fl_value_set_string_take(dock, "anchorLeft", fl_value_new_bool(!preview_ &&
        gtk_layer_get_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_LEFT)));
    fl_value_set_string_take(dock, "anchorRight", fl_value_new_bool(!preview_ &&
        gtk_layer_get_anchor(dock_window_, GTK_LAYER_SHELL_EDGE_RIGHT)));
    gint requested_width = 0;
    gint requested_height = 0;
    gtk_widget_get_size_request(GTK_WIDGET(dock_window_), &requested_width, &requested_height);
    fl_value_set_string_take(dock, "requestedWidth", fl_value_new_int(requested_width));
    fl_value_set_string_take(dock, "requestedHeight", fl_value_new_int(requested_height));
    fl_value_set_string_take(state, "dock", g_steal_pointer(&dock));
  }
  fl_value_set_string_take(state, "blurAvailable", fl_value_new_bool(
      main_effect_ != nullptr && main_effect_->Available()));
  fl_value_set_string_take(state, "barBlurRequested", fl_value_new_bool(
      macos_ && !closed_ && !(expanded_ && launcher_backdrop_) &&
      main_effect_ != nullptr && main_effect_->Requested()));
  fl_value_set_string_take(state, "panelBlurRequested", fl_value_new_bool(
      macos_ && expanded_ && !closed_ && panel_glass_region_.has_value()));
  g_autoptr(FlValue) panel_glass = fl_value_new_null();
  if (panel_glass_region_) {
    g_clear_pointer(&panel_glass, fl_value_unref);
    panel_glass = fl_value_new_map();
    fl_value_set_string_take(panel_glass, "x", fl_value_new_float(panel_glass_region_->x));
    fl_value_set_string_take(panel_glass, "y", fl_value_new_float(panel_glass_region_->y));
    fl_value_set_string_take(panel_glass, "width", fl_value_new_float(panel_glass_region_->width));
    fl_value_set_string_take(panel_glass, "height", fl_value_new_float(panel_glass_region_->height));
    fl_value_set_string_take(panel_glass, "radius", fl_value_new_float(panel_glass_region_->radius));
  }
  fl_value_set_string_take(state, "panelGlassRegion", g_steal_pointer(&panel_glass));
  g_autoptr(FlValue) main_blur_regions = fl_value_new_list();
  if (main_effect_ != nullptr) {
    for (const auto& rect : main_effect_->AppliedRegions()) {
      g_autoptr(FlValue) entry = fl_value_new_map();
      fl_value_set_string_take(entry, "x", fl_value_new_int(rect.x));
      fl_value_set_string_take(entry, "y", fl_value_new_int(rect.y));
      fl_value_set_string_take(entry, "width", fl_value_new_int(rect.width));
      fl_value_set_string_take(entry, "height", fl_value_new_int(rect.height));
      fl_value_append_take(main_blur_regions, g_steal_pointer(&entry));
    }
  }
  fl_value_set_string_take(state, "mainBlurRegions", g_steal_pointer(&main_blur_regions));
  fl_value_set_string_take(state, "launcherBackdropRequested", fl_value_new_bool(
      macos_ && expanded_ && !closed_ && launcher_backdrop_ &&
      main_effect_ != nullptr && main_effect_->Requested()));
  fl_value_set_string_take(state, "launcherSceneVisible", fl_value_new_bool(
      macos_ && launcher_backdrop_ && !closed_));
  fl_value_set_string_take(state, "dockBlurRequested", fl_value_new_bool(
      dock_effect_ != nullptr && dock_effect_->Requested()));
  fl_value_set_string_take(state, "expanded", fl_value_new_bool(expanded_));
  fl_value_set_string_take(state, "tooltipVisible", fl_value_new_bool(tooltip_visible_));
  fl_value_set_string_take(state, "barPosition", fl_value_new_string(
      BarBottom() ? "bottom" : "top"));
  fl_value_set_string_take(state, "menuBarAutoHide", fl_value_new_bool(mac_menu_auto_hide_));
  fl_value_set_string_take(state, "menuBarVisible", fl_value_new_bool(mac_menu_visible_ || expanded_));
  fl_value_set_string_take(state, "menuBarWidthPercent", fl_value_new_float(mac_menu_width_percent_));
  fl_value_set_string_take(state, "menuBarHeight", fl_value_new_int(mac_menu_height_));
  fl_value_set_string_take(state, "preview", fl_value_new_bool(preview_));
  fl_value_set_string_take(state, "width", fl_value_new_int(
      gtk_widget_get_allocated_width(GTK_WIDGET(window_))));
  fl_value_set_string_take(state, "height", fl_value_new_int(
      gtk_widget_get_allocated_height(GTK_WIDGET(window_))));
  const int input_height = gtk_widget_get_allocated_height(GTK_WIDGET(window_));
  fl_value_set_string_take(state, "inputY", fl_value_new_int(InputY(input_height)));
  fl_value_set_string_take(state, "inputHeight", fl_value_new_int(
      preview_ || expanded_ ? input_height :
      clarion_shell::BarInputHeight(macos_, mac_menu_auto_hide_, mac_menu_visible_, input_height, mac_menu_height_)));
  fl_value_set_string_take(state, "layer", fl_value_new_string(
      preview_ ? "Preview" : (gtk_layer_get_layer(window_) ==
          GTK_LAYER_SHELL_LAYER_OVERLAY ? "Overlay" : "Top")));
  const GtkLayerShellKeyboardMode mode = preview_
      ? GTK_LAYER_SHELL_KEYBOARD_MODE_NONE
      : gtk_layer_get_keyboard_mode(window_);
  fl_value_set_string_take(state, "keyboard", fl_value_new_string(
      mode == GTK_LAYER_SHELL_KEYBOARD_MODE_EXCLUSIVE ? "Exclusive" : "None"));
  fl_value_set_string_take(state, "keyboardInteractivityOverride",
      keyboard_interactivity_override_.has_value()
          ? fl_value_new_string(*keyboard_interactivity_override_ ? "exclusive" : "none")
          : fl_value_new_null());
  fl_value_set_string_take(state, "exclusiveZone", fl_value_new_int(
      preview_ ? 0 : gtk_layer_get_exclusive_zone(window_)));
  fl_value_set_string_take(state, "reservationZone", fl_value_new_int(
      macos_ ? (menu_reservation_ != nullptr && gtk_widget_get_visible(GTK_WIDGET(menu_reservation_))
          ? gtk_layer_get_exclusive_zone(menu_reservation_) : 0)
      : (preview_ ? 0 : gtk_layer_get_exclusive_zone(window_))));
  fl_value_set_string_take(state, "anchorTop", fl_value_new_bool(
      !preview_ && gtk_layer_get_anchor(
          window_, GTK_LAYER_SHELL_EDGE_TOP)));
  fl_value_set_string_take(state, "anchorBottom", fl_value_new_bool(
      !preview_ && gtk_layer_get_anchor(
          window_, GTK_LAYER_SHELL_EDGE_BOTTOM)));
  fl_value_set_string_take(state, "anchorLeft", fl_value_new_bool(
      !preview_ && gtk_layer_get_anchor(window_, GTK_LAYER_SHELL_EDGE_LEFT)));
  fl_value_set_string_take(state, "anchorRight", fl_value_new_bool(
      !preview_ && gtk_layer_get_anchor(window_, GTK_LAYER_SHELL_EDGE_RIGHT)));
  fl_value_set_string_take(state, "focused", fl_value_new_bool(
      gtk_window_is_active(window_)));
  const GdkRectangle geometry = MonitorGeometry();
  g_autoptr(FlValue) monitor = fl_value_new_map();
  fl_value_set_string_take(monitor, "x", fl_value_new_int(geometry.x));
  fl_value_set_string_take(monitor, "y", fl_value_new_int(geometry.y));
  fl_value_set_string_take(monitor, "width", fl_value_new_int(geometry.width));
  fl_value_set_string_take(monitor, "height", fl_value_new_int(geometry.height));
  fl_value_set_string_take(state, "monitorGeometry", g_steal_pointer(&monitor));
  gint requested_width = 0;
  gint requested_height = 0;
  gtk_widget_get_size_request(GTK_WIDGET(window_),
                              &requested_width, &requested_height);
  fl_value_set_string_take(state, "requestedHeight", fl_value_new_int(requested_height));
  fl_value_set_string_take(state, "scaleFactor", fl_value_new_int(
      gtk_widget_get_scale_factor(GTK_WIDGET(window_))));
  return g_steal_pointer(&state);
}

void WindowController::MethodCall(FlMethodChannel*, FlMethodCall* call,
                                 gpointer data) {
  auto* self = static_cast<WindowController*>(data);
  const gchar* method = fl_method_call_get_name(call);
  FlValue* arguments = fl_method_call_get_args(call);
  g_autoptr(FlMethodResponse) response = nullptr;
  if (strcmp(method, "setExpanded") == 0) {
    if (arguments == nullptr || fl_value_get_type(arguments) != FL_VALUE_TYPE_BOOL) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setExpanded requires a boolean", nullptr));
    } else if (!self->SetExpanded(fl_value_get_bool(arguments))) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setKeyboardInteractivity") == 0) {
    const gchar* mode = arguments != nullptr &&
        fl_value_get_type(arguments) == FL_VALUE_TYPE_STRING
        ? fl_value_get_string(arguments) : nullptr;
    if (mode == nullptr || (strcmp(mode, "none") != 0 &&
        strcmp(mode, "exclusive") != 0 && strcmp(mode, "default") != 0)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setKeyboardInteractivity requires none, exclusive or default", nullptr));
    } else {
      const std::optional<bool> exclusive = strcmp(mode, "default") == 0
          ? std::nullopt : std::optional<bool>(strcmp(mode, "exclusive") == 0);
      if (!self->SetKeyboardInteractivity(exclusive)) {
        response = FL_METHOD_RESPONSE(fl_method_error_response_new(
            "window-closed", "The shell window has closed", nullptr));
      } else {
        response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
      }
    }
  } else if (strcmp(method, "setLauncherBackdrop") == 0) {
    const bool map = arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP;
    FlValue* visible = map ? fl_value_lookup_string(arguments, "visible") : nullptr;
    FlValue* interactive = map ? fl_value_lookup_string(arguments, "dockInteractive") : nullptr;
    if (visible == nullptr || fl_value_get_type(visible) != FL_VALUE_TYPE_BOOL ||
        interactive == nullptr || fl_value_get_type(interactive) != FL_VALUE_TYPE_BOOL) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setLauncherBackdrop requires boolean visible and dockInteractive", nullptr));
    } else if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      self->launcher_backdrop_ = self->macos_ && fl_value_get_bool(visible);
      self->launcher_dock_interactive_ = self->launcher_backdrop_ && fl_value_get_bool(interactive);
      if (self->launcher_backdrop_) self->panel_glass_region_.reset();
      self->ConfigureDockWindow();
      self->UpdateBackgroundEffects();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setTooltipVisible") == 0) {
    const bool map = arguments != nullptr &&
        fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP;
    FlValue* visible = map ? fl_value_lookup_string(arguments, "visible") : arguments;
    const gchar* role = map ? SurfaceRole(arguments) : "main";
    if (visible == nullptr || fl_value_get_type(visible) != FL_VALUE_TYPE_BOOL ||
        role == nullptr || (strcmp(role, "main") != 0 && strcmp(role, "dock") != 0)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setTooltipVisible requires a boolean or surface/visible map", nullptr));
    } else if (!self->SetTooltipVisible(fl_value_get_bool(visible), strcmp(role, "dock") == 0)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setShellStyle") == 0) {
    const gchar* style = arguments != nullptr &&
        fl_value_get_type(arguments) == FL_VALUE_TYPE_STRING
        ? fl_value_get_string(arguments) : nullptr;
    if (style == nullptr ||
        (strcmp(style, "material") != 0 && strcmp(style, "macos") != 0)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setShellStyle requires material or macos", nullptr));
    } else if (!self->SetShellStyle(strcmp(style, "macos") == 0)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          self->closed_ ? "window-closed" : "dock-unavailable",
          "Unable to create a Dock view; Material remains available", nullptr));
    } else {
      g_autoptr(FlValue) state = self->WindowState();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(state));
    }
  } else if (strcmp(method, "setMacMenuBarPreferences") == 0) {
    const bool map = arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP;
    FlValue* auto_hide = map ? fl_value_lookup_string(arguments, "autoHide") : nullptr;
    FlValue* visible = map ? fl_value_lookup_string(arguments, "visible") : nullptr;
    double width_percent = 0;
    // Old callers have no height field and retain the Catalina default.
    FlValue* height_value = map ? fl_value_lookup_string(arguments, "height") : nullptr;
    double menu_height = clarion_shell::kMacosBarHeight;
    if (!map ||
        auto_hide == nullptr || fl_value_get_type(auto_hide) != FL_VALUE_TYPE_BOOL ||
        visible == nullptr || fl_value_get_type(visible) != FL_VALUE_TYPE_BOOL ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "widthPercent"), &width_percent) ||
        width_percent < 50 || width_percent > 100 ||
        (height_value != nullptr && !ReadFiniteNumber(height_value, &menu_height)) ||
        std::floor(menu_height) != menu_height ||
        menu_height < clarion_shell::kMacosBarHeight ||
        menu_height > clarion_shell::kMaxMacosBarHeight) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "Menu bar preferences require boolean autoHide/visible, widthPercent between 50 and 100 and integer height between 22 and 48", nullptr));
    } else if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      self->mac_menu_auto_hide_ = fl_value_get_bool(auto_hide);
      self->mac_menu_visible_ = !self->mac_menu_auto_hide_ || fl_value_get_bool(visible);
      self->mac_menu_width_percent_ = width_percent;
      self->mac_menu_height_ = static_cast<int>(menu_height);
      self->ConfigureLayerWindow();
      self->UpdateInputRegion();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setDockGeometry") == 0) {
    int width = 0, height = 0, reserved = 0;
    const bool map = arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP;
    FlValue* position = map ? fl_value_lookup_string(arguments, "position") : nullptr;
    FlValue* auto_hide = map ? fl_value_lookup_string(arguments, "autoHide") : nullptr;
    const gchar* text = position != nullptr && fl_value_get_type(position) == FL_VALUE_TYPE_STRING
        ? fl_value_get_string(position) : nullptr;
    if (!map || text == nullptr ||
        (strcmp(text, "bottom") != 0 && strcmp(text, "left") != 0 && strcmp(text, "right") != 0) ||
        !ReadCoordinate(fl_value_lookup_string(arguments, "width"), &width) ||
        !ReadCoordinate(fl_value_lookup_string(arguments, "height"), &height) ||
        !ReadCoordinate(fl_value_lookup_string(arguments, "reservedSize"), &reserved) ||
        width < 1 || height < 1 || reserved < 0 || reserved > 256 ||
        auto_hide == nullptr || fl_value_get_type(auto_hide) != FL_VALUE_TYPE_BOOL) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "Dock geometry requires position, positive size, bounded reservation and autoHide", nullptr));
    } else if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      self->dock_position_ = strcmp(text, "left") == 0 ? clarion_shell::DockPosition::Left
          : strcmp(text, "right") == 0 ? clarion_shell::DockPosition::Right
                                       : clarion_shell::DockPosition::Bottom;
      self->dock_desired_width_ = width;
      self->dock_desired_height_ = height;
      self->dock_reserved_size_ = reserved;
      self->dock_auto_hide_ = fl_value_get_bool(auto_hide);
      self->ConfigureDockWindow();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setDockWidth") == 0) {
    int width = 0;
    if (!ReadCoordinate(arguments, &width) || width < 1) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setDockWidth requires a positive finite width", nullptr));
    } else if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      self->dock_desired_width_ = width;
      self->ConfigureDockWindow();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setDockInputRegion") == 0) {
    clarion_shell::InputRect rect = {0, 0, 0, 0};
    std::optional<std::vector<clarion_shell::InputRect>> regions;
    bool valid = ReadDockInputRect(arguments, &rect);
    FlValue* entries = valid ? fl_value_lookup_string(arguments, "regions") : nullptr;
    if (entries != nullptr) {
      valid = fl_value_get_type(entries) == FL_VALUE_TYPE_LIST;
      if (valid) {
        regions.emplace();
        for (size_t index = 0; index < fl_value_get_length(entries); ++index) {
          clarion_shell::InputRect entry;
          if (!ReadDockInputRect(fl_value_get_list_value(entries, index), &entry)) {
            valid = false;
            break;
          }
          regions->push_back(entry);
        }
      }
    }
    if (!valid) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setDockInputRegion requires finite non-negative rectangles", nullptr));
    } else if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      self->dock_input_ = rect;
      self->dock_input_regions_ = std::move(regions);
      self->dock_custom_input_ = true;
      self->UpdateDockInputRegion();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setPanelGlassRegion") == 0) {
    clarion_shell::GlassRegion requested, normalized;
    if (arguments == nullptr || fl_value_get_type(arguments) != FL_VALUE_TYPE_MAP ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "x"), &requested.x) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "y"), &requested.y) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "width"), &requested.width) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "height"), &requested.height) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "radius"), &requested.radius) ||
        !clarion_shell::NormalizeGlassRegion(requested, &normalized)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setPanelGlassRegion requires finite bounded coordinates and non-negative size/radius", nullptr));
    } else if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      // The caller reports after setExpanded completes. Ignore delayed reports
      // from a closed panel so the next expansion cannot resurrect old glass.
      std::optional<clarion_shell::GlassRegion> next;
      if (self->macos_ && self->expanded_ && normalized.width > 0 && normalized.height > 0) {
        next = normalized;
      }
      if (!(next == self->panel_glass_region_)) {
        self->panel_glass_region_ = next;
        self->UpdateBackgroundEffects();
      }
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setDockGlassRegion") == 0) {
    double x, y, width, height, radius;
    if (arguments == nullptr || fl_value_get_type(arguments) != FL_VALUE_TYPE_MAP ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "x"), &x) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "y"), &y) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "width"), &width) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "height"), &height) ||
        !ReadFiniteNumber(fl_value_lookup_string(arguments, "radius"), &radius) ||
        width < 0 || height < 0 || radius < 0) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setDockGlassRegion requires a finite glass rectangle and radius", nullptr));
    } else {
      self->dock_glass_x_ = x; self->dock_glass_y_ = y;
      self->dock_glass_width_ = width; self->dock_glass_height_ = height;
      self->dock_glass_radius_ = radius;
      self->UpdateBackgroundEffects();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "setBarPosition") == 0) {
    const gchar* position = arguments != nullptr &&
        fl_value_get_type(arguments) == FL_VALUE_TYPE_STRING
        ? fl_value_get_string(arguments) : nullptr;
    if (position == nullptr ||
        (strcmp(position, "top") != 0 && strcmp(position, "bottom") != 0)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "setBarPosition requires top or bottom", nullptr));
    } else if (!self->SetBarPosition(strcmp(position, "bottom") == 0)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
    }
  } else if (strcmp(method, "outputName") == 0) {
    g_autoptr(FlValue) value = self->output_name_ == nullptr
        ? fl_value_new_null() : fl_value_new_string(self->output_name_);
    response = FL_METHOD_RESPONSE(fl_method_success_response_new(value));
  } else if (strcmp(method, "surfaceOrigin") == 0) {
    const gchar* role = SurfaceRole(arguments);
    const bool dock = role != nullptr && strcmp(role, "dock") == 0;
    GtkWindow* target = dock ? self->dock_window_ : self->window_;
    if (role == nullptr || (strcmp(role, "main") != 0 && !dock)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "surfaceOrigin requires main or dock", nullptr));
    } else if (target == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      // Flutter's global positions are relative to this surface. SNI actions
      // take screen coordinates. Compute the selected output's current
      // logical origin and the bottom-anchored surface offset at action time,
      // so resizing, relocation and hot unplug cannot leave a cached origin.
      const GdkRectangle geometry = self->preview_
          ? GdkRectangle{0, 0, 0, 0} : self->MonitorGeometry();
      int surface_y = self->BarBottom() && !self->preview_
          ? std::max(0, geometry.height - gtk_widget_get_allocated_height(
              GTK_WIDGET(target))) : 0;
      int surface_x = 0;
      if (dock && !self->preview_) {
        const auto placement = clarion_shell::PlaceDock(self->dock_position_,
            gtk_widget_get_allocated_width(GTK_WIDGET(target)),
            gtk_widget_get_allocated_height(GTK_WIDGET(target)), geometry.width, geometry.height);
        surface_x = placement.x;
        surface_y = placement.y;
      } else if (!self->preview_ && self->macos_ && !self->expanded_) {
        surface_x = std::max(0, (geometry.width - gtk_widget_get_allocated_width(GTK_WIDGET(target))) / 2);
      }
      g_autoptr(FlValue) origin = fl_value_new_map();
      fl_value_set_string_take(origin, "x", fl_value_new_int(geometry.x + surface_x));
      fl_value_set_string_take(origin, "y", fl_value_new_int(geometry.y + surface_y));
      fl_value_set_string_take(origin, "outputX", fl_value_new_int(surface_x));
      fl_value_set_string_take(origin, "outputY", fl_value_new_int(surface_y));
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(origin));
    }
  } else if (strcmp(method, "windowState") == 0) {
    if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else {
      g_autoptr(FlValue) state = self->WindowState();
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(state));
    }
  } else if (strcmp(method, "appIcon") == 0) {
    const gchar* icon_name = nullptr;
    const gchar* theme_path = nullptr;
    int icon_size = kIconSize;
    int pixel_size = 0;
    bool valid_pixel_size = true;
    if (arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_STRING) {
      icon_name = fl_value_get_string(arguments);
    } else if (arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP) {
      FlValue* name = fl_value_lookup_string(arguments, "name");
      FlValue* path = fl_value_lookup_string(arguments, "themePath");
      FlValue* size = fl_value_lookup_string(arguments, "size");
      FlValue* pixels = fl_value_lookup_string(arguments, "pixelSize");
      const auto pixel_kind = pixels == nullptr
          ? clarion_shell::IconPixelSizeKind::kAbsent
          : fl_value_get_type(pixels) == FL_VALUE_TYPE_INT
              ? clarion_shell::IconPixelSizeKind::kInteger
              : clarion_shell::IconPixelSizeKind::kInvalid;
      valid_pixel_size = clarion_shell::ParseIconPixelSize(
          pixel_kind, pixel_kind == clarion_shell::IconPixelSizeKind::kInteger
              ? static_cast<double>(fl_value_get_int(pixels)) : 0, &pixel_size);
      if (name != nullptr && fl_value_get_type(name) == FL_VALUE_TYPE_STRING) {
        icon_name = fl_value_get_string(name);
      }
      if (path != nullptr && fl_value_get_type(path) == FL_VALUE_TYPE_STRING) {
        theme_path = fl_value_get_string(path);
      }
      if (size != nullptr && fl_value_get_type(size) == FL_VALUE_TYPE_INT) {
        icon_size = static_cast<int>(std::max<int64_t>(
            16, std::min<int64_t>(128, fl_value_get_int(size))));
      }
    }
    if (self->window_ == nullptr || self->closed_) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "window-closed", "The shell window has closed", nullptr));
    } else if (!valid_pixel_size || icon_name == nullptr || strlen(icon_name) > 512 ||
               (theme_path != nullptr && strlen(theme_path) > 4096)) {
      response = FL_METHOD_RESPONSE(fl_method_error_response_new(
          "invalid-argument", "appIcon requires an icon name, bounded theme path, and integer pixelSize from 16 to 512", nullptr));
    } else {
      GtkIconTheme* theme = gtk_icon_theme_get_for_screen(
          gtk_widget_get_screen(GTK_WIDGET(self->window_)));
      g_autoptr(GtkIconTheme) custom_theme = nullptr;
      if (theme_path != nullptr && *theme_path != '\0') {
        // Add application-supplied hicolor/size/status directories only to this
        // lookup. A tray application must not mutate the shared desktop theme.
        custom_theme = gtk_icon_theme_new();
        gtk_icon_theme_set_screen(custom_theme,
            gtk_widget_get_screen(GTK_WIDGET(self->window_)));
        gtk_icon_theme_prepend_search_path(custom_theme, theme_path);
        theme = custom_theme;
      }
      g_autoptr(GtkIconInfo) icon = clarion_shell::LookupAppIcon(
          theme, icon_name, icon_size,
          gtk_widget_get_scale_factor(GTK_WIDGET(self->window_)),
          pixel_size);
      const gchar* filename = icon == nullptr ? nullptr : gtk_icon_info_get_filename(icon);
      g_autoptr(FlValue) value = filename == nullptr
          ? fl_value_new_null() : fl_value_new_string(filename);
      response = FL_METHOD_RESPONSE(fl_method_success_response_new(value));
    }
  } else {
    response = FL_METHOD_RESPONSE(fl_method_not_implemented_response_new());
  }
  Respond(call, response);
}
