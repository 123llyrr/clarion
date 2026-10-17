// SPDX-License-Identifier: GPL-3.0-or-later
// A native pointer route for the isolated launcher composition test only.
#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <wayland-client.h>

#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"

static struct zwlr_virtual_pointer_manager_v1 *manager;

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    (void)version;
    if (strcmp(interface, "zwlr_virtual_pointer_manager_v1") == 0) {
        manager = wl_registry_bind(registry, name,
            &zwlr_virtual_pointer_manager_v1_interface, 1);
    }
}

static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}

static uint32_t timestamp(void) {
    struct timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return (uint32_t)((uint64_t)value.tv_sec * 1000 + value.tv_nsec / 1000000);
}

static int number(const char *text, uint32_t *value) {
    char *end;
    long parsed = strtol(text, &end, 10);
    if (!*text || *end || parsed < 0 || parsed > 32768) return 0;
    *value = (uint32_t)parsed;
    return 1;
}

int main(int argc, char **argv) {
    const char *isolated = getenv("CLARION_SHELL_ISOLATED_TEST");
    const char *display = getenv("WAYLAND_DISPLAY");
    const char *parent = getenv("CLARION_SHELL_PARENT_WAYLAND_DISPLAY");
    uint32_t x, y, width, height;
    if (!isolated || strcmp(isolated, "1") || !display || !parent ||
        !strcmp(display, parent)) {
        fputs("Pointer fixture requires a distinct isolated test display.\n", stderr);
        return 2;
    }
    if (argc != 5 || !number(argv[1], &x) || !number(argv[2], &y) ||
        !number(argv[3], &width) || !number(argv[4], &height) ||
        width == 0 || height == 0 || x >= width || y >= height) {
        fputs("Usage: launcher-spike-pointer X Y OUTPUT_WIDTH OUTPUT_HEIGHT\n", stderr);
        return 2;
    }
    struct wl_display *connection = wl_display_connect(NULL);
    if (!connection) return 1;
    struct wl_registry *registry = wl_display_get_registry(connection);
    const struct wl_registry_listener listener = {global, removed};
    wl_registry_add_listener(registry, &listener, NULL);
    if (wl_display_roundtrip(connection) < 0 || !manager) {
        fputs("The test compositor has no virtual pointer manager.\n", stderr);
        wl_display_disconnect(connection);
        return 1;
    }
    struct zwlr_virtual_pointer_v1 *pointer =
        zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
    zwlr_virtual_pointer_v1_motion_absolute(pointer, timestamp(), x, y, width, height);
    zwlr_virtual_pointer_v1_frame(pointer);
    if (wl_display_roundtrip(connection) < 0) return 1;
    zwlr_virtual_pointer_v1_button(pointer, timestamp(), 0x110, WL_POINTER_BUTTON_STATE_PRESSED);
    zwlr_virtual_pointer_v1_frame(pointer);
    if (wl_display_roundtrip(connection) < 0) return 1;
    zwlr_virtual_pointer_v1_button(pointer, timestamp(), 0x110, WL_POINTER_BUTTON_STATE_RELEASED);
    zwlr_virtual_pointer_v1_frame(pointer);
    int result = wl_display_roundtrip(connection);
    zwlr_virtual_pointer_v1_destroy(pointer);
    zwlr_virtual_pointer_manager_v1_destroy(manager);
    wl_registry_destroy(registry);
    wl_display_flush(connection);
    wl_display_disconnect(connection);
    return result < 0 ? 1 : 0;
}
