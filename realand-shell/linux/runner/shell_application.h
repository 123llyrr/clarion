// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef REALAND_SHELL_APPLICATION_H_
#define REALAND_SHELL_APPLICATION_H_

#include <gtk/gtk.h>

G_DECLARE_FINAL_TYPE(ShellApplication, shell_application, REALAND,
                     SHELL_APPLICATION, GtkApplication)

ShellApplication* shell_application_new();

#endif  // REALAND_SHELL_APPLICATION_H_
