// SPDX-License-Identifier: GPL-3.0-or-later
#include "shell_application.h"
#include "startup_trace.h"

int main(int argc, char** argv) {
  realand_shell::TraceStartup("native-entry");
  g_autoptr(ShellApplication) app = shell_application_new();
  return g_application_run(G_APPLICATION(app), argc, argv);
}
