// SPDX-License-Identifier: GPL-3.0-or-later
// Compile with pkg-config gtk+-3.0; run only on a private bus/nested compositor.
#include <gtk/gtk.h>
#include <string.h>

static const char *result_path;
static unsigned int activations;

static void verify(GSimpleAction *action, GVariant *parameter, gpointer data) {
  (void)action;
  (void)parameter;
  (void)data;
  char *text = g_strdup_printf("%u\n", ++activations);
  g_printerr("Real GTK app.verify activated: %u\n", activations);
  GError *error = NULL;
  if (!g_file_set_contents(result_path, text, -1, &error)) {
    g_printerr("Cannot write private menu result: %s\n", error->message);
    g_clear_error(&error);
  }
  g_free(text);
}

static void startup(GApplication *application, gpointer data) {
  (void)data;
  const GActionEntry entries[] = {
      {.name = "verify", .activate = verify},
      {.name = "disabled", .activate = verify},
  };
  g_action_map_add_action_entries(G_ACTION_MAP(application), entries,
                                G_N_ELEMENTS(entries), NULL);
  g_simple_action_set_enabled(G_SIMPLE_ACTION(g_action_map_lookup_action(
                                  G_ACTION_MAP(application), "disabled")),
                              FALSE);
  GMenu *file = g_menu_new();
  g_menu_append(file, "验证动作", "app.verify");
  GMenu *edit = g_menu_new();
  g_menu_append(edit, "不可用", "app.disabled");
  GMenu *bar = g_menu_new();
  g_menu_append_submenu(bar, "_File", G_MENU_MODEL(file));
  g_menu_append_submenu(bar, "_编辑", G_MENU_MODEL(edit));
  gtk_application_set_menubar(GTK_APPLICATION(application), G_MENU_MODEL(bar));
  g_object_unref(bar);
  g_object_unref(file);
  g_object_unref(edit);
}

static GtkWidget *window(GtkApplication *application, const char *title) {
  GtkWidget *result = gtk_application_window_new(application);
  gtk_window_set_title(GTK_WINDOW(result), title);
  gtk_window_set_default_size(GTK_WINDOW(result), 520, 260);
  // capabilities=0 keeps the application's local menubar available as well.
  gtk_application_window_set_show_menubar(GTK_APPLICATION_WINDOW(result), TRUE);
  gtk_container_add(GTK_CONTAINER(result),
                    gtk_label_new("Clarion private GTK global menu fixture"));
  gtk_widget_show_all(result);
  return result;
}

static void activate(GApplication *application, gpointer data) {
  (void)data;
  if (gtk_application_get_windows(GTK_APPLICATION(application)) != NULL) return;
  GtkWidget *first = window(GTK_APPLICATION(application),
                            "Clarion GTK Menu Fixture");
  window(GTK_APPLICATION(application), "Clarion GTK Second Window");
  gtk_window_present(GTK_WINDOW(first));
}

int main(int argc, char **argv) {
  const char *isolated = g_getenv("CLARION_SHELL_ISOLATED_TEST");
  const char *wayland = g_getenv("WAYLAND_DISPLAY");
  const char *parent_wayland = g_getenv("CLARION_SHELL_PARENT_WAYLAND_DISPLAY");
  const char *bus = g_getenv("DBUS_SESSION_BUS_ADDRESS");
  const char *parent_bus = g_getenv("CLARION_SHELL_PARENT_DBUS_SESSION_BUS_ADDRESS");
  const char *socket = g_getenv("CLARION_SOCKET");
  const char *parent_socket = g_getenv("CLARION_SHELL_PARENT_SOCKET");
  result_path = g_getenv("CLARION_MENU_QA_RESULT");
  if (g_strcmp0(isolated, "1") != 0 || !wayland || !parent_wayland ||
      g_strcmp0(wayland, parent_wayland) == 0 || !bus || !parent_bus ||
      g_strcmp0(bus, parent_bus) == 0 || !socket || !parent_socket ||
      g_strcmp0(socket, parent_socket) == 0 || !result_path ||
      !g_path_is_absolute(result_path)) {
    g_printerr("Refusing non-isolated GTK menu QA environment\n");
    return 2;
  }
  if (!g_file_set_contents(result_path, "0\n", -1, NULL)) return 3;
  // GDK may derive xdg_toplevel.app_id from the executable's program name.
  // Keep this fixture deterministic even when its compiled binary is renamed.
  g_set_prgname("org.test.ClarionMenuFixture");
  GtkApplication *application =
      gtk_application_new("org.test.ClarionMenuFixture", 0);
  g_signal_connect(application, "startup", G_CALLBACK(startup), NULL);
  g_signal_connect(application, "activate", G_CALLBACK(activate), NULL);
  int status = g_application_run(G_APPLICATION(application), argc, argv);
  g_object_unref(application);
  return status;
}
