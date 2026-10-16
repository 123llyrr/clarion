// SPDX-License-Identifier: GPL-3.0-or-later
#include "shell_application.h"
#include "startup_trace.h"

int main(int argc, char** argv) {
  // Import legacy overrides before GTK or Flutter creates worker threads.
  // Explicit Clarion values, including empty ones, remain authoritative.
  g_auto(GStrv) variables = g_listenv();
  for (size_t index = 0; variables[index] != nullptr; ++index) {
    const char* legacy = variables[index];
    if (!g_str_has_prefix(legacy, "REALAND_")) continue;
    g_autofree char* canonical = g_strconcat("CLARION_", legacy + 8, nullptr);
    if (g_getenv(canonical) == nullptr) {
      g_setenv(canonical, g_getenv(legacy), FALSE);
    }
  }
  clarion_shell::TraceStartup("native-entry");
  g_autoptr(ShellApplication) app = shell_application_new();
  return g_application_run(G_APPLICATION(app), argc, argv);
}
