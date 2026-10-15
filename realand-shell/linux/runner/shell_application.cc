// SPDX-License-Identifier: GPL-3.0-or-later
#include "shell_application.h"

#include <flutter_linux/flutter_linux.h>
#include <gtk-layer-shell.h>

#include <cerrno>
#include <climits>
#include <cstdlib>
#include <cstring>

#include "flutter/generated_plugin_registrant.h"
#include "desktop_windows.h"
#include "shell_geometry.h"
#include "startup_trace.h"
#include "window_controller.h"

struct _ShellApplication {
  GtkApplication parent_instance;
  gchar** dart_arguments;
  gchar* output_name;
  gint monitor_index;
  gboolean preview;
  gboolean desktop_only;
  gboolean desktop_preview;
  gboolean no_desktop;
  GtkWindow* window;
  WindowController* controller;
  DesktopWindows* desktops;
};

G_DEFINE_TYPE(ShellApplication, shell_application, GTK_TYPE_APPLICATION)

static void first_frame_cb(ShellApplication*, FlView* view) {
  realand_shell::TraceStartup("main-first-frame");
  GtkWidget* window = gtk_widget_get_toplevel(GTK_WIDGET(view));
  gtk_widget_show(window);
}

static void shell_application_activate(GApplication* application) {
  realand_shell::TraceStartup("native-activate");
  auto* self = REALAND_SHELL_APPLICATION(application);
  if (self->desktop_only) {
    if (self->desktops == nullptr) {
      self->desktops = new DesktopWindows(GTK_APPLICATION(application),
          self->dart_arguments, self->desktop_preview, self->monitor_index,
          self->output_name);
      self->desktops->Start();
    }
    return;
  }
  if (self->window != nullptr) {
    if (self->preview) gtk_window_present(self->window);
    return;
  }
  self->window = GTK_WINDOW(
      gtk_application_window_new(GTK_APPLICATION(application)));
  // The appmenu GTK module can install an empty application menu model. Its
  // native menubar still paints a themed separator over our Flutter surface.
  // Shell UI is entirely Flutter; leave client applications' menu export intact.
  gtk_application_window_set_show_menubar(
      GTK_APPLICATION_WINDOW(self->window), FALSE);
  g_object_add_weak_pointer(G_OBJECT(self->window),
                           reinterpret_cast<gpointer*>(&self->window));
  gtk_window_set_title(self->window, "Realand Shell");
  gtk_window_set_default_size(self->window, self->preview ? 1200 : 1,
                               self->preview ? 800 : realand_shell::kBarHeight);
  gtk_window_set_decorated(self->window, self->preview);
  gtk_widget_set_app_paintable(GTK_WIDGET(self->window), TRUE);
  GdkVisual* visual = gdk_screen_get_rgba_visual(
      gtk_widget_get_screen(GTK_WIDGET(self->window)));
  if (visual != nullptr) gtk_widget_set_visual(GTK_WIDGET(self->window), visual);

  // The view supplies per-pixel alpha. Do not let a GTK theme paint a solid
  // background behind it or advertise the transparent window as opaque.
  g_autoptr(GtkCssProvider) css = gtk_css_provider_new();
  gtk_css_provider_load_from_data(css,
      "window { background-color: transparent; background-image: none; }",
      -1, nullptr);
  gtk_style_context_add_provider(
      gtk_widget_get_style_context(GTK_WIDGET(self->window)),
      GTK_STYLE_PROVIDER(css), GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(project, self->dart_arguments);
  realand_shell::TraceStartup("flutter-view-create-begin");
  FlView* view = fl_view_new(project);
  realand_shell::TraceStartup("flutter-view-create-end");
  const GdkRGBA transparent = {0.0, 0.0, 0.0, 0.0};
  fl_view_set_background_color(view, &transparent);
  gtk_container_add(GTK_CONTAINER(self->window), GTK_WIDGET(view));

  GdkDisplay* display = gtk_widget_get_display(GTK_WIDGET(self->window));
  GdkMonitor* monitor = self->monitor_index >= 0
      ? gdk_display_get_monitor(display, self->monitor_index)
      : gdk_display_get_primary_monitor(display);
  if (monitor == nullptr && gdk_display_get_n_monitors(display) > 0) {
    monitor = gdk_display_get_monitor(display, 0);
  }
  delete self->controller;
  self->controller = new WindowController(
      self->window, view, self->preview, monitor, self->output_name);
  fl_register_plugins(FL_PLUGIN_REGISTRY(view));
  g_signal_connect_swapped(view, "first-frame", G_CALLBACK(first_frame_cb), self);
  gtk_widget_show(GTK_WIDGET(view));
  gtk_widget_realize(GTK_WIDGET(view));
  if (!self->preview && !self->no_desktop && self->desktops == nullptr) {
    self->desktops = new DesktopWindows(GTK_APPLICATION(application),
                                        self->dart_arguments);
    self->desktops->Start();
  }
}

static void print_usage() {
  g_print("Realand Shell\n"
          "Usage: realand-shell [--preview|--demo] [--monitor=N] [--output-name=NAME] "
          "[Dart options]\n"
          "  --preview          Open a regular 1200x800 test window\n"
          "  --demo             Use preview mode with demo data\n"
          "  --desktop-only     Create desktop views without a bar\n"
          "  --desktop-preview  Open a regular desktop test window without a bar\n"
          "  --no-desktop       Disable attached desktop views\n"
          "  --monitor=N        Use the zero-based GTK monitor index\n"
          "  --output-name=NAME Identify the selected output to the IPC client\n"
          "  --no-tray          Disable StatusNotifier tray hosting\n"
          "  --no-notifications Disable the notification server\n"
          "  --help             Show this help\n");
}

static gboolean shell_application_local_command_line(
    GApplication* application, gchar*** arguments, int* exit_status) {
  auto* self = REALAND_SHELL_APPLICATION(application);
  self->no_desktop = g_strcmp0(g_getenv("REALAND_SHELL_DISABLE_DESKTOP"), "1") == 0;
  self->desktop_only =
      g_strcmp0(g_getenv("REALAND_SHELL_ISOLATED_TEST"), "1") == 0 &&
      g_strcmp0(g_getenv("REALAND_SHELL_TEST_DESKTOP_ONLY"), "1") == 0;
  gboolean explicitly_disabled = FALSE;
  for (gchar** argument = *arguments + 1; *argument != nullptr; ++argument) {
    if (strcmp(*argument, "--help") == 0 || strcmp(*argument, "-h") == 0) {
      print_usage();
      *exit_status = 0;
      return TRUE;
    }
    if (strcmp(*argument, "--desktop-preview") == 0) {
      self->desktop_only = TRUE;
      self->desktop_preview = TRUE;
      self->preview = TRUE;
    } else if (strcmp(*argument, "--desktop-only") == 0 ||
               strcmp(*argument, "--desktop") == 0) {
      self->desktop_only = TRUE;
    } else if (strcmp(*argument, "--no-desktop") == 0) {
      explicitly_disabled = TRUE;
      self->no_desktop = TRUE;
    } else if (strcmp(*argument, "--preview") == 0 || strcmp(*argument, "--demo") == 0) {
      self->preview = TRUE;
    } else if (g_str_has_prefix(*argument, "--monitor=") ||
               strcmp(*argument, "--monitor") == 0) {
      const gchar* value = g_str_has_prefix(*argument, "--monitor=")
          ? *argument + strlen("--monitor=") : *(++argument);
      if (value == nullptr) {
        g_printerr("--monitor requires a zero-based output index\n");
        *exit_status = 2;
        return TRUE;
      }
      char* end = nullptr;
      errno = 0;
      const long index = strtol(value, &end, 10);
      gboolean digits_only = *value != '\0';
      for (const gchar* digit = value; *digit != '\0'; ++digit) {
        digits_only = digits_only && g_ascii_isdigit(*digit);
      }
      if (errno != 0 || *value == '\0' || *end != '\0' || index < 0 ||
          index > INT_MAX || !digits_only) {
        g_printerr("Invalid monitor index: %s\n", value);
        *exit_status = 2;
        return TRUE;
      }
      self->monitor_index = static_cast<gint>(index);
    } else if (g_str_has_prefix(*argument, "--output-name=")) {
      const gchar* value = *argument + strlen("--output-name=");
      if (*value == '\0' || strlen(value) > 512 || !g_utf8_validate(value, -1, nullptr)) {
        g_printerr("Output name must be nonempty UTF-8, at most 512 bytes\n");
        *exit_status = 2;
        return TRUE;
      }
      g_free(self->output_name);
      self->output_name = g_strdup(value);
    }
  }

  if (explicitly_disabled && self->desktop_only) {
    g_printerr("--no-desktop cannot be combined with desktop-only mode\n");
    *exit_status = 2;
    return TRUE;
  }
  if (self->desktop_only && self->preview) self->desktop_preview = TRUE;

  g_autoptr(GError) error = nullptr;
  realand_shell::TraceStartup("gtk-register-begin");
  if (!g_application_register(application, nullptr, &error)) {
    realand_shell::TraceStartup("gtk-register-failed");
    g_printerr("Unable to start Realand Shell: %s\n", error->message);
    *exit_status = 1;
    return TRUE;
  }
  realand_shell::TraceStartup("gtk-register-end");
  if (!self->preview && !gtk_layer_is_supported()) {
    g_printerr("Realand Shell requires Wayland layer-shell. "
               "Use --preview for a regular window.\n");
    *exit_status = 1;
    return TRUE;
  }
  GdkDisplay* display = gdk_display_get_default();
  if (self->monitor_index >= gdk_display_get_n_monitors(display)) {
    g_printerr("Monitor %d does not exist (%d outputs available)\n",
               self->monitor_index, gdk_display_get_n_monitors(display));
    *exit_status = 2;
    return TRUE;
  }
  // Argument vector boundaries are preserved; no command string or shell
  // expansion is involved in passing user options to Dart.
  g_clear_pointer(&self->dart_arguments, g_strfreev);
  self->dart_arguments = g_strdupv(*arguments + 1);
  g_application_activate(application);
  *exit_status = 0;
  return TRUE;
}

static void shell_application_dispose(GObject* object) {
  auto* self = REALAND_SHELL_APPLICATION(object);
  delete self->desktops;
  self->desktops = nullptr;
  delete self->controller;
  self->controller = nullptr;
  if (self->window != nullptr) {
    g_object_remove_weak_pointer(G_OBJECT(self->window),
                                reinterpret_cast<gpointer*>(&self->window));
    self->window = nullptr;
  }
  g_clear_pointer(&self->dart_arguments, g_strfreev);
  g_clear_pointer(&self->output_name, g_free);
  G_OBJECT_CLASS(shell_application_parent_class)->dispose(object);
}

static void shell_application_class_init(ShellApplicationClass* klass) {
  G_APPLICATION_CLASS(klass)->activate = shell_application_activate;
  G_APPLICATION_CLASS(klass)->local_command_line =
      shell_application_local_command_line;
  G_OBJECT_CLASS(klass)->dispose = shell_application_dispose;
}

static void shell_application_init(ShellApplication* self) {
  self->monitor_index = -1;
}

ShellApplication* shell_application_new() {
  g_set_prgname(APPLICATION_ID);
  return REALAND_SHELL_APPLICATION(g_object_new(
      shell_application_get_type(), "application-id", APPLICATION_ID,
      "flags", G_APPLICATION_NON_UNIQUE, nullptr));
}
