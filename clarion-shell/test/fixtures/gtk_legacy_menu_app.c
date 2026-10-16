// SPDX-License-Identifier: GPL-3.0-or-later
// Traditional GtkWindow/GtkMenuBar: exports no menu on its own. This fixture
// therefore exercises the real appmenu-gtk-module rather than GTK GMenu setup.
#include <gtk/gtk.h>
#include <gdk/gdkwayland.h>
#include <string.h>

static const char *result_path;
static const char *state_path;
static GtkWidget *window;
static GtkWidget *menubar;
static GtkWidget *count_label;
static GSettings *settings;
static unsigned int activations;
static gboolean checked;

static void write_count(void) {
  char *text = g_strdup_printf("%u\n", activations);
  GError *error = NULL;
  if (!g_file_set_contents(result_path, text, -1, &error)) {
    g_printerr("Cannot write private legacy menu count: %s\n", error->message);
    g_clear_error(&error);
  }
  gtk_label_set_text(GTK_LABEL(count_label), text);
  g_free(text);
}

static gboolean write_state(gpointer data) {
  (void)data;
  gboolean shell_shows_menubar = FALSE;
  g_object_get(gtk_settings_get_default(), "gtk-shell-shows-menubar",
               &shell_shows_menubar, NULL);
  char *text = g_strdup_printf(
      "{\"activations\":%u,\"checked\":%s,\"plainGtkWindow\":true,"
      "\"isGtkApplicationWindow\":%s,\"realized\":%s,\"mapped\":%s,"
      "\"wayland\":%s,\"menubarWidth\":%d,\"menubarHeight\":%d,"
      "\"shellShowsMenubar\":%s,\"alwaysShowInnerMenu\":%s,"
      "\"runOnWayland\":%s}\n",
      activations, checked ? "true" : "false",
      GTK_IS_APPLICATION_WINDOW(window) ? "true" : "false",
      gtk_widget_get_realized(window) ? "true" : "false",
      gtk_widget_get_mapped(window) ? "true" : "false",
      GDK_IS_WAYLAND_DISPLAY(gdk_display_get_default()) ? "true" : "false",
      gtk_widget_get_allocated_width(menubar),
      gtk_widget_get_allocated_height(menubar),
      shell_shows_menubar ? "true" : "false",
      g_settings_get_boolean(settings, "always-show-inner-menu") ? "true" : "false",
      g_settings_get_boolean(settings, "run-on-wayland") ? "true" : "false");
  GError *error = NULL;
  if (!g_file_set_contents(state_path, text, -1, &error)) {
    g_printerr("Cannot write private legacy menu state: %s\n", error->message);
    g_clear_error(&error);
  }
  g_free(text);
  return G_SOURCE_CONTINUE;
}

static void verify(GtkMenuItem *item, gpointer data) {
  (void)item;
  (void)data;
  activations++;
  write_count();
  write_state(NULL);
}

static void toggle(GtkCheckMenuItem *item, gpointer data) {
  (void)data;
  checked = gtk_check_menu_item_get_active(item);
  write_state(NULL);
}

static gboolean different_environment(const char *key, const char *parent_key) {
  const char *current = g_getenv(key);
  const char *parent = g_getenv(parent_key);
  return current && *current && parent && *parent &&
         g_strcmp0(current, parent) != 0;
}

int main(int argc, char **argv) {
  const char *module = g_getenv("GTK_MODULES");
  const char *schema_dir = g_getenv("GSETTINGS_SCHEMA_DIR");
  result_path = g_getenv("CLARION_MENU_QA_RESULT");
  state_path = g_getenv("CLARION_LEGACY_MENU_QA_STATE");
  if (g_strcmp0(g_getenv("CLARION_SHELL_ISOLATED_TEST"), "1") != 0 ||
      !different_environment("WAYLAND_DISPLAY", "CLARION_SHELL_PARENT_WAYLAND_DISPLAY") ||
      !different_environment("DBUS_SESSION_BUS_ADDRESS", "CLARION_SHELL_PARENT_DBUS_SESSION_BUS_ADDRESS") ||
      !different_environment("CLARION_SOCKET", "CLARION_SHELL_PARENT_SOCKET") ||
      g_strcmp0(g_getenv("GSETTINGS_BACKEND"), "memory") != 0 ||
      !module || !strstr(module, "appmenu-gtk-module") ||
      !schema_dir || !g_path_is_absolute(schema_dir) ||
      !result_path || !g_path_is_absolute(result_path) ||
      !state_path || !g_path_is_absolute(state_path)) {
    g_printerr("Refusing non-isolated legacy GTK module QA environment\n");
    return 2;
  }

  // Set the module's private in-memory policy before gtk_init loads modules.
  // This never reads or writes the user's persistent dconf preferences.
  GSettingsSchemaSource *source = g_settings_schema_source_get_default();
  GSettingsSchema *schema = source
      ? g_settings_schema_source_lookup(source, "org.appmenu.gtk-module", TRUE)
      : NULL;
  if (!schema) {
    g_printerr("Private appmenu-gtk-module schema is unavailable\n");
    return 3;
  }
  settings = g_settings_new_full(schema, NULL, NULL);
  g_settings_schema_unref(schema);
  if (!g_settings_set_boolean(settings, "always-show-inner-menu", TRUE) ||
      !g_settings_set_boolean(settings, "run-on-wayland", TRUE)) {
    g_printerr("Cannot set private in-memory module policy\n");
    g_object_unref(settings);
    return 3;
  }
  g_setenv("GDK_BACKEND", "wayland", TRUE);
  if (!gtk_init_check(&argc, &argv) ||
      !GDK_IS_WAYLAND_DISPLAY(gdk_display_get_default())) {
    g_printerr("Cannot initialize the private GTK Wayland display\n");
    g_object_unref(settings);
    return 4;
  }

  window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
  gtk_window_set_title(GTK_WINDOW(window), "Clarion Legacy GTK Menu Fixture");
  gtk_window_set_default_size(GTK_WINDOW(window), 520, 260);
  GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
  gtk_container_add(GTK_CONTAINER(window), box);
  menubar = gtk_menu_bar_new();
  gtk_box_pack_start(GTK_BOX(box), menubar, FALSE, FALSE, 0);
  GtkWidget *file = gtk_menu_item_new_with_mnemonic("_File");
  GtkWidget *menu = gtk_menu_new();
  gtk_menu_item_set_submenu(GTK_MENU_ITEM(file), menu);
  gtk_menu_shell_append(GTK_MENU_SHELL(menubar), file);
  GtkWidget *count = gtk_menu_item_new_with_mnemonic("_Increase counter");
  g_signal_connect(count, "activate", G_CALLBACK(verify), NULL);
  gtk_menu_shell_append(GTK_MENU_SHELL(menu), count);
  GtkWidget *disabled = gtk_menu_item_new_with_mnemonic("_Disabled");
  gtk_widget_set_sensitive(disabled, FALSE);
  g_signal_connect(disabled, "activate", G_CALLBACK(verify), NULL);
  gtk_menu_shell_append(GTK_MENU_SHELL(menu), disabled);
  GtkWidget *check = gtk_check_menu_item_new_with_mnemonic("_Checked");
  gtk_menu_shell_append(GTK_MENU_SHELL(menu), check);
  count_label = gtk_label_new("0");
  gtk_box_pack_start(GTK_BOX(box), count_label, TRUE, TRUE, 0);
  g_signal_connect(check, "toggled", G_CALLBACK(toggle), NULL);
  g_signal_connect(window, "destroy", G_CALLBACK(gtk_main_quit), NULL);
  write_count();
  gtk_widget_show_all(window);
  write_state(NULL);
  const guint timer = g_timeout_add(250, write_state, NULL);
  gtk_main();
  g_source_remove(timer);
  g_object_unref(settings);
  return 0;
}
