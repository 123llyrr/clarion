// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_APPLICATION_H_
#define CLARION_SHELL_APPLICATION_H_

#include <gtk/gtk.h>

G_DECLARE_FINAL_TYPE(ShellApplication, shell_application, CLARION,
                     SHELL_APPLICATION, GtkApplication)

ShellApplication* shell_application_new();

#endif  // CLARION_SHELL_APPLICATION_H_
