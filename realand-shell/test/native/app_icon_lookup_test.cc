// SPDX-License-Identifier: GPL-3.0-or-later
// Run without Flutter:
// c++ -std=c++14 -Wall -Wextra -Werror test/native/app_icon_lookup_test.cc
//   $(pkg-config --cflags --libs gtk+-3.0) -o /tmp/realand-app-icon-test
// /tmp/realand-app-icon-test
#include "../../linux/runner/app_icon_lookup.h"

#include <glib/gstdio.h>

#include <cassert>
#include <iostream>
#include <limits>
#include <string>

namespace {

void RemoveDirectory(const std::string& path) {
  GDir* dir = g_dir_open(path.c_str(), 0, nullptr);
  assert(dir != nullptr);
  while (const gchar* name = g_dir_read_name(dir)) {
    const std::string child = path + "/" + name;
    if (g_file_test(child.c_str(), G_FILE_TEST_IS_DIR)) {
      RemoveDirectory(child);
    } else {
      assert(g_remove(child.c_str()) == 0);
    }
  }
  g_dir_close(dir);
  assert(g_rmdir(path.c_str()) == 0);
}

struct Fixture {
  Fixture() {
    g_autofree gchar* temporary = g_dir_make_tmp("realand-icon-test-XXXXXX", nullptr);
    assert(temporary != nullptr);
    root = temporary;
  }
  ~Fixture() { RemoveDirectory(root); }

  void Write(const std::string& relative, const std::string& contents) const {
    const std::string path = root + "/" + relative;
    g_autofree gchar* parent = g_path_get_dirname(path.c_str());
    assert(g_mkdir_with_parents(parent, 0700) == 0);
    assert(g_file_set_contents(path.c_str(), contents.c_str(), contents.size(),
                               nullptr));
  }

  void Theme(const std::string& name, const std::string& inherits = "") const {
    Write(name + "/index.theme",
          "[Icon Theme]\nName=" + name + "\nComment=Temporary test theme\n"
          "Inherits=" + inherits + "\n"
          "Directories=48x48/apps,48x48@2/apps,128x128/apps,256x256/apps,scalable/apps\n"
          "[48x48/apps]\nSize=48\nType=Fixed\nContext=Applications\n"
          "[48x48@2/apps]\nSize=48\nScale=2\nType=Fixed\nContext=Applications\n"
          "[128x128/apps]\nSize=128\nType=Threshold\nThreshold=2\nContext=Applications\n"
          "[256x256/apps]\nSize=256\nType=Fixed\nContext=Applications\n"
          "[scalable/apps]\nSize=48\nType=Scalable\nMinSize=16\nMaxSize=512\nContext=Applications\n");
  }

  void Png(const std::string& relative, int pixels) const {
    const std::string path = root + "/" + relative;
    g_autofree gchar* parent = g_path_get_dirname(path.c_str());
    assert(g_mkdir_with_parents(parent, 0700) == 0);
    g_autoptr(GdkPixbuf) pixbuf = gdk_pixbuf_new(
        GDK_COLORSPACE_RGB, true, 8, pixels, pixels);
    assert(pixbuf != nullptr);
    gdk_pixbuf_fill(pixbuf, 0x3399ffff);
    assert(gdk_pixbuf_save(pixbuf, path.c_str(), "png", nullptr, nullptr));
  }

  void Svg(const std::string& relative) const {
    Write(relative, "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"48\" height=\"48\" "
                    "viewBox=\"0 0 48 48\"><rect width=\"48\" height=\"48\" "
                    "fill=\"#3399ff\"/></svg>");
  }

  GtkIconTheme* CreateTheme(const char* name) const {
    GtkIconTheme* theme = gtk_icon_theme_new();
    const gchar* paths[] = {root.c_str()};
    gtk_icon_theme_set_search_path(theme, paths, 1);
    gtk_icon_theme_set_custom_theme(theme, name);
    return theme;
  }

  void Expect(GtkIconTheme* theme, const char* name, int logical, int scale,
              int pixels, const std::string& expected) const {
    g_autoptr(GtkIconInfo) icon = realand_shell::LookupAppIcon(
        theme, name, logical, scale, pixels);
    assert(icon != nullptr);
    const gchar* filename = gtk_icon_info_get_filename(icon);
    assert(filename != nullptr);
    if (filename != root + "/" + expected) {
      std::cerr << "Expected " << expected << ", got " << filename << '\n';
      assert(false);
    }
    std::cout << name << " logical=" << logical << " gtkScale=" << scale
              << " pixelSize=" << pixels << " -> " << expected << '\n';
  }

  std::string root;
};

void TestPixelSizeValidation() {
  using realand_shell::IconPixelSizeKind;
  using realand_shell::ParseIconPixelSize;
  int pixels = -1;
  assert(ParseIconPixelSize(IconPixelSizeKind::kAbsent, 0, &pixels) && pixels == 0);
  for (double valid : {16.0, 48.0, 129.0, 512.0}) {
    assert(ParseIconPixelSize(IconPixelSizeKind::kInteger, valid, &pixels));
    assert(pixels == valid);
  }
  for (double invalid : {-1.0, 0.0, 15.0, 513.0, 1e30, 48.5,
                         std::numeric_limits<double>::quiet_NaN(),
                         std::numeric_limits<double>::infinity()}) {
    pixels = 77;
    assert(!ParseIconPixelSize(IconPixelSizeKind::kInteger, invalid, &pixels));
    assert(pixels == 77);
  }
  // The FlValue bridges mark null, string, bool, and every float (including
  // integral floats and NaN) invalid rather than treating them as omitted.
  for (double value : {0.0, 48.0, std::numeric_limits<double>::quiet_NaN()}) {
    assert(!ParseIconPixelSize(IconPixelSizeKind::kInvalid, value, &pixels));
  }
}

void TestThemeFiles() {
  Fixture fixture;
  fixture.Theme("Selected", "Inherited");
  fixture.Theme("Inherited");
  fixture.Theme("hicolor");
  for (int size : {48, 128, 256}) {
    fixture.Png("Selected/" + std::to_string(size) + "x" +
                    std::to_string(size) + "/apps/raster.png", size);
  }
  fixture.Png("Selected/48x48@2/apps/raster.png", 96);
  fixture.Png("Selected/48x48/apps/vector.png", 48);
  fixture.Svg("Selected/scalable/apps/vector.svg");
  fixture.Png("Selected/48x48/apps/style.png", 48);
  fixture.Svg("Inherited/scalable/apps/style.svg");
  fixture.Png("Selected/48x48/apps/regular.png", 48);
  fixture.Svg("Selected/scalable/apps/regular-symbolic.svg");
  fixture.Png("Selected/128x128/apps/mislabelled.png", 48);
  fixture.Png("Selected/256x256/apps/mislabelled.png", 256);
  fixture.Png("Selected/48x48/apps/small-only.png", 48);
  fixture.Svg("Inherited/scalable/apps/inherited-only.svg");

  g_autoptr(GtkIconTheme) theme = fixture.CreateTheme("Selected");
  fixture.Expect(theme, "raster", 48, 1, 0, "Selected/48x48/apps/raster.png");
  fixture.Expect(theme, "raster", 48, 2, 0, "Selected/48x48@2/apps/raster.png");
  fixture.Expect(theme, "raster", 48, 1, 144, "Selected/256x256/apps/raster.png");
  fixture.Expect(theme, "raster", 48, 2, 144, "Selected/256x256/apps/raster.png");
  fixture.Expect(theme, "raster", 48, 2, 48, "Selected/48x48/apps/raster.png");
  fixture.Expect(theme, "raster", 48, 2, 128, "Selected/128x128/apps/raster.png");
  fixture.Expect(theme, "raster", 128, 1, 129, "Selected/256x256/apps/raster.png");
  fixture.Expect(theme, "vector", 48, 2, 144, "Selected/scalable/apps/vector.svg");
  fixture.Expect(theme, "style", 48, 2, 144, "Selected/48x48/apps/style.png");
  fixture.Expect(theme, "regular", 48, 2, 144, "Selected/48x48/apps/regular.png");
  fixture.Expect(theme, "mislabelled", 48, 2, 96, "Selected/256x256/apps/mislabelled.png");
  fixture.Expect(theme, "small-only", 48, 2, 512, "Selected/48x48/apps/small-only.png");
  fixture.Expect(theme, "inherited-only", 48, 2, 144, "Inherited/scalable/apps/inherited-only.svg");
  assert(realand_shell::LookupAppIcon(theme, "missing", 48, 2, 144) == nullptr);
  assert(realand_shell::LookupAppIcon(theme, "raster", 48, 2, 15) == nullptr);
  assert(realand_shell::LookupAppIcon(theme, "raster", 48, 2, 513) == nullptr);

  // A custom lookup can prepend its root on an isolated theme without changing
  // either the selected theme's paths or GTK's shared per-screen theme.
  GdkScreen* screen = gdk_screen_get_default();
  GtkIconTheme* shared_theme = screen == nullptr ? nullptr
      : gtk_icon_theme_get_for_screen(screen);
  gchar** shared_paths_before = nullptr;
  int shared_path_count_before = 0;
  if (shared_theme != nullptr) {
    gtk_icon_theme_get_search_path(shared_theme, &shared_paths_before,
                                   &shared_path_count_before);
  }
  g_autoptr(GtkIconTheme) isolated = fixture.CreateTheme("Selected");
  gtk_icon_theme_prepend_search_path(isolated, (fixture.root + "/extra").c_str());
  fixture.Expect(isolated, "raster", 48, 2, 144, "Selected/256x256/apps/raster.png");
  gchar** original_paths = nullptr;
  int original_path_count = 0;
  gtk_icon_theme_get_search_path(theme, &original_paths, &original_path_count);
  assert(original_path_count == 1 && fixture.root == original_paths[0]);
  g_strfreev(original_paths);
  if (shared_theme != nullptr) {
    g_autofree gchar* selected_name_before = nullptr;
    g_object_get(gtk_settings_get_for_screen(screen), "gtk-icon-theme-name",
                 &selected_name_before, nullptr);
    assert(selected_name_before != nullptr);
    // Exercise the runners' exact set_screen + prepend_search_path sequence,
    // using a unique name only present in the temporary application theme.
    fixture.Theme(selected_name_before, "hicolor");
    const std::string selected = selected_name_before;
    fixture.Png(selected + "/48x48/apps/realand-fixture-path.png", 48);
    fixture.Png(selected + "/256x256/apps/realand-fixture-path.png", 256);
    g_autoptr(GtkIconTheme) application_theme = gtk_icon_theme_new();
    gtk_icon_theme_set_screen(application_theme, screen);
    gtk_icon_theme_prepend_search_path(application_theme, fixture.root.c_str());
    fixture.Expect(application_theme, "realand-fixture-path", 48, 2, 144,
                   selected + "/256x256/apps/realand-fixture-path.png");
    fixture.Expect(application_theme, "realand-fixture-path", 48, 1, 0,
                   selected + "/48x48/apps/realand-fixture-path.png");
    g_autofree gchar* selected_name_after = nullptr;
    g_object_get(gtk_settings_get_for_screen(screen), "gtk-icon-theme-name",
                 &selected_name_after, nullptr);
    assert(selected == selected_name_after);
    gchar** shared_paths_after = nullptr;
    int shared_path_count_after = 0;
    gtk_icon_theme_get_search_path(shared_theme, &shared_paths_after,
                                   &shared_path_count_after);
    assert(shared_path_count_before == shared_path_count_after);
    for (int index = 0; index < shared_path_count_before; ++index) {
      assert(std::string(shared_paths_before[index]) == shared_paths_after[index]);
    }
    g_strfreev(shared_paths_after);
    g_strfreev(shared_paths_before);
  }
}

}  // namespace

int main(int argc, char** argv) {
  // No windows are created. Custom themes also work without a display; when a
  // display exists the shared theme is only read to verify isolation.
  gtk_init_check(&argc, &argv);
  TestPixelSizeValidation();
  TestThemeFiles();
  std::cout << "app icon lookup tests passed\n";
}
