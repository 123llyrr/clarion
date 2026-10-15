// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef REALAND_SHELL_STARTUP_TRACE_H_
#define REALAND_SHELL_STARTUP_TRACE_H_

#include <glib.h>

namespace realand_shell {

// Opt-in timing covers GTK startup before Dart and the first Flutter frame.
// Only this process reads the flag; no session environment is modified.
inline void TraceStartup(const char* stage) {
  static const bool enabled =
      g_strcmp0(g_getenv("REALAND_SHELL_STARTUP_TRACE"), "1") == 0;
  if (!enabled) return;
  static const gint64 entered = g_get_monotonic_time();
  const gint64 now = g_get_monotonic_time();
  g_message("Realand Shell startup: stage=%s monotonic_us=%" G_GINT64_FORMAT
            " elapsed_ms=%.3f",
            stage, now, static_cast<double>(now - entered) / 1000.0);
}

}  // namespace realand_shell

#endif  // REALAND_SHELL_STARTUP_TRACE_H_
