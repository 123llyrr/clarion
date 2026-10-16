// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_DESKTOP_WINDOWS_H_
#define CLARION_SHELL_DESKTOP_WINDOWS_H_

#include <gtk/gtk.h>

#include <memory>

// Owns one independent Flutter desktop view per monitor on the GTK main thread.
// Removing a monitor tears down only its view, engine and method channels.
class DesktopWindows final {
 public:
  DesktopWindows(GtkApplication* application, gchar** dart_arguments,
                 bool preview = false, gint preview_monitor = -1,
                 const gchar* preview_output_name = nullptr);
  ~DesktopWindows();
  DesktopWindows(const DesktopWindows&) = delete;
  DesktopWindows& operator=(const DesktopWindows&) = delete;

  void Start();

 private:
  class Impl;
  std::unique_ptr<Impl> impl_;
};

#endif  // CLARION_SHELL_DESKTOP_WINDOWS_H_
