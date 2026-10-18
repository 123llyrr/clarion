// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_WINDOW_CONTROLLER_H_
#define CLARION_SHELL_WINDOW_CONTROLLER_H_

#include <flutter_linux/flutter_linux.h>
#include <gtk/gtk.h>
#include <memory>
#include <optional>
#include <vector>

#include "shell_geometry.h"
#include "background_effect.h"
#include "native_tray_menu.h"
#include "native_clipboard.h"

// Lives on the GTK main thread. The Flutter channel owns no window reference,
// so closing a preview or losing an output cannot leave callbacks with a stale
// GtkWindow. Destroy this controller before disposing the application.
class WindowController final {
 public:
  WindowController(GtkWindow* window, FlView* view, bool preview,
                   GdkMonitor* monitor, const gchar* output_name);
  ~WindowController();
  WindowController(const WindowController&) = delete;
  WindowController& operator=(const WindowController&) = delete;

 private:
  static void MethodCall(FlMethodChannel*, FlMethodCall*, gpointer);
  static void SizeAllocated(GtkWidget*, GtkAllocation*, gpointer);
  static void MonitorGeometryChanged(GObject*, GParamSpec*, gpointer);
  static void MonitorRemoved(GdkDisplay*, GdkMonitor*, gpointer);
  static void StyleUpdated(GtkWidget*, gpointer);
  static void WindowDestroyed(GtkWidget*, gpointer);
  static void DockSizeAllocated(GtkWidget*, GtkAllocation*, gpointer);
  static void DockStyleUpdated(GtkWidget*, gpointer);
  static void DockFirstFrame(FlView*, gpointer);
  static gboolean DockReadyTimeout(gpointer);
  static gboolean DockDeleteEvent(GtkWidget*, GdkEvent*, gpointer);
  bool SetExpanded(bool expanded);
  bool SetKeyboardInteractivity(std::optional<bool> exclusive);
  bool MainKeyboardExclusive() const;
  bool SetTooltipVisible(bool visible, bool dock = false);
  bool SetBarPosition(bool bottom);
  bool SetShellStyle(bool macos);
  bool EnsureDock();
  void DestroyDock();
  void ConfigureDockWindow();
  bool EnsureReservationWindow(GtkWindow** window, const gchar* name);
  void ConfigureReservations();
  void DestroyReservations();
  static void ReservationInput(GtkWidget*, gpointer);
  static void ReservationSizeAllocated(GtkWidget*, GtkAllocation*, gpointer);
  static gboolean ReservationDraw(GtkWidget*, cairo_t*, gpointer);
  void UpdateDockInputRegion();
  void ClearDockOpaqueRegion();
  FlValue* WindowState() const;
  void DismissTooltip();
  void ConfigureLayerWindow();
  void UpdateInputRegion();
  void SetMonitor(GdkMonitor* monitor);
  void ClearOpaqueRegion();
  void UpdateBackgroundEffects();
  void PublishOutputGeometry();
  GdkRectangle MonitorGeometry() const;
  int InputY(int height) const;
  int BarHeight() const;
  bool BarBottom() const;

  GtkWindow* window_ = nullptr;  // Weak pointer, cleared by GObject.
  FlView* view_ = nullptr;      // Weak pointer, cleared by GObject.
  GtkWindow* dock_window_ = nullptr;  // Owned GTK child window, weak pointer.
  FlView* dock_view_ = nullptr;       // Owned by dock_window_, weak pointer.
  GtkWindow* menu_reservation_ = nullptr;
  GtkWindow* dock_reservation_ = nullptr;
  GdkDisplay* display_ = nullptr;
  GdkMonitor* monitor_ = nullptr;
  FlMethodChannel* channel_ = nullptr;
  gchar* output_name_ = nullptr;
  bool preview_ = false;
  bool expanded_ = false;
  // Expanded normally owns the keyboard. An external overlay can temporarily
  // own it while this surface continues drawing its full-output backdrop.
  std::optional<bool> keyboard_interactivity_override_;
  bool native_tray_menu_keyboard_ = false;
  std::unique_ptr<NativeTrayMenu> native_tray_menu_;
  std::unique_ptr<NativeClipboardBridge> native_clipboard_;
  bool tooltip_visible_ = false;
  bool bar_bottom_ = false;
  bool macos_ = false;
  bool mac_menu_auto_hide_ = false;
  bool mac_menu_visible_ = true;
  bool launcher_backdrop_ = false;
  bool launcher_dock_interactive_ = false;
  double mac_menu_width_percent_ = 100;
  int mac_menu_height_ = clarion_shell::kMacosBarHeight;
  bool closed_ = false;
  bool dock_first_frame_ = false;
  bool dock_tooltip_visible_ = false;
  int dock_desired_width_ = clarion_shell::kDefaultDockWidth;
  int dock_desired_height_ = clarion_shell::kDockSurfaceHeight;
  // Reserve nothing until Dart reports the Dock it is actually drawing. A
  // Dock hidden from startup never sends geometry, so a nonzero default would
  // keep an exclusive strip along the edge with no Dock to justify it.
  int dock_reserved_size_ = 0;
  bool dock_auto_hide_ = false;
  clarion_shell::DockPosition dock_position_ = clarion_shell::DockPosition::Bottom;
  bool dock_custom_input_ = false;
  std::unique_ptr<BackgroundEffect> main_effect_;
  std::unique_ptr<BackgroundEffect> dock_effect_;
  std::optional<clarion_shell::GlassRegion> panel_glass_region_;
  double dock_glass_x_ = 0, dock_glass_y_ = 0, dock_glass_width_ = 0,
         dock_glass_height_ = 0, dock_glass_radius_ = 12;
  clarion_shell::InputRect dock_input_ = {0, 20, 720, 100};
  std::optional<std::vector<clarion_shell::InputRect>> dock_input_regions_;
  std::vector<clarion_shell::InputRect> dock_applied_input_regions_;
  gulong size_handler_ = 0;
  gulong style_handler_ = 0;
  gulong geometry_handler_ = 0;
  gulong monitor_scale_handler_ = 0;
  gulong removed_handler_ = 0;
  gulong destroy_handler_ = 0;
  gulong dock_size_handler_ = 0;
  gulong dock_style_handler_ = 0;
  gulong dock_first_frame_handler_ = 0;
  gulong dock_delete_handler_ = 0;
  guint dock_ready_timeout_ = 0;
};

#endif  // CLARION_SHELL_WINDOW_CONTROLLER_H_
