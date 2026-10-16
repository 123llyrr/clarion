// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_APP_ICON_LOOKUP_H_
#define CLARION_SHELL_APP_ICON_LOOKUP_H_

#include <gtk/gtk.h>

#include <algorithm>
#include <cmath>
#include <vector>

namespace clarion_shell {

enum class IconPixelSizeKind { kAbsent, kInteger, kInvalid };

// pixelSize is a physical pixel count, not another logical size. Reject a
// present value of the wrong type instead of silently falling back to legacy
// scale handling. The double also makes finite/range checks safe before casts.
inline bool ParseIconPixelSize(IconPixelSizeKind kind, double number,
                               int* pixel_size) {
  if (kind == IconPixelSizeKind::kAbsent) {
    *pixel_size = 0;
    return true;
  }
  if (kind != IconPixelSizeKind::kInteger || !std::isfinite(number) ||
      number < 16 || number > 512 || std::floor(number) != number) {
    return false;
  }
  *pixel_size = static_cast<int>(number);
  return true;
}

inline bool IconFileMeetsPixelSize(GtkIconInfo* icon, int pixel_size) {
  const gchar* filename = gtk_icon_info_get_filename(icon);
  if (filename == nullptr) return false;
  if (g_str_has_suffix(filename, ".svg")) return true;
  int width = 0;
  int height = 0;
  // Directory Size/Scale metadata is not necessarily the file's real size.
  // Inspect the source without decoding or manufacturing a larger bitmap.
  return gdk_pixbuf_get_file_info(filename, &width, &height) != nullptr &&
         std::max(width, height) >= pixel_size;
}

// Returns an owned GtkIconInfo. Theme inheritance, search-path priority and
// regular/symbolic selection stay with GTK; no differently named icon is tried.
inline GtkIconInfo* LookupAppIcon(GtkIconTheme* theme, const gchar* icon_name,
                                  int logical_size, int gtk_scale,
                                  int pixel_size) {
  if (pixel_size == 0) {
    // Preserve tray callers that only supply the legacy logical size.
    return gtk_icon_theme_lookup_icon_for_scale(
        theme, icon_name, logical_size, gtk_scale, GTK_ICON_LOOKUP_FORCE_SIZE);
  }
  if (pixel_size < 16 || pixel_size > 512) return nullptr;

  constexpr auto flags = static_cast<GtkIconLookupFlags>(
      GTK_ICON_LOOKUP_FORCE_SIZE | GTK_ICON_LOOKUP_FORCE_SVG);
  // Flutter already included its DPR. Passing GTK's scale here would count it
  // twice. FORCE_SVG allows vector files even without a GTK SVG loader; it does
  // not force an SVG or change the icon's name/style.
  g_autoptr(GtkIconInfo) icon = gtk_icon_theme_lookup_icon_for_scale(
      theme, icon_name, pixel_size, 1, flags);
  if (icon == nullptr || IconFileMeetsPixelSize(icon, pixel_size)) {
    return g_steal_pointer(&icon);
  }

  // GTK usually prefers downscaling, but a threshold directory can count a
  // slightly smaller raster as an exact match. Probe the theme's other sizes
  // before accepting that upscale. Every lookup still uses GTK's first theme
  // containing this name, so an inherited high-res icon cannot replace the
  // current theme's application artwork. -1 denotes a scalable source.
  g_autofree gint* available_sizes = gtk_icon_theme_get_icon_sizes(theme, icon_name);
  std::vector<int> probes{512};
  if (available_sizes != nullptr) {
    for (const gint* size = available_sizes; *size != 0; ++size) {
      if (*size > 0) probes.push_back(*size);
    }
  }
  std::sort(probes.begin(), probes.end());
  probes.erase(std::unique(probes.begin(), probes.end()), probes.end());
  for (int size : probes) {
    if (size == pixel_size) continue;
    g_autoptr(GtkIconInfo) candidate = gtk_icon_theme_lookup_icon_for_scale(
        theme, icon_name, size, 1, flags);
    if (candidate != nullptr &&
        gtk_icon_info_is_symbolic(candidate) == gtk_icon_info_is_symbolic(icon) &&
        IconFileMeetsPixelSize(candidate, pixel_size)) {
      return g_steal_pointer(&candidate);
    }
  }
  // No adequate source exists in the selected theme. Return the real original
  // source; never claim a resized pixbuf or another app icon is higher detail.
  return g_steal_pointer(&icon);
}

}  // namespace clarion_shell

#endif  // CLARION_SHELL_APP_ICON_LOOKUP_H_
