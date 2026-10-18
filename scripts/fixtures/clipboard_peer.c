/* SPDX-License-Identifier: GPL-3.0-or-later
 * Disposable ext-data-control peer. Refuses every non-private Wayland socket.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include "ext-data-control-v1-client-protocol.h"

#define LIMIT (18u * 1024u * 1024u)
struct fixture;
struct offer {
  struct fixture* fixture;
  struct ext_data_control_offer_v1* proxy;
  char mimes[128][256];
  int count;
};
struct fixture {
  struct wl_display* display;
  struct wl_registry* registry;
  struct wl_seat* seat;
  struct ext_data_control_manager_v1* manager;
  struct ext_data_control_device_v1* device;
  struct ext_data_control_source_v1* source;
  struct offer* offers[64];
  int offer_count, done, failed, offer_mode, primary, sensitive, screenshot;
  const char *mime, *file, *ready, *requests;
  unsigned char* bytes;
  size_t size;
};
static void fail(const char* reason) { fprintf(stderr, "%s\n", reason); exit(2); }
static int prefix(const char* value, const char* root) {
  return value && root && strncmp(value, root, strlen(root)) == 0 && value[strlen(root)] == '/';
}
static void guard(const char* path) {
  const char *runtime = getenv("XDG_RUNTIME_DIR"), *root = getenv("CLARION_CLIPBOARD_TEST_ROOT");
  const char *display = getenv("WAYLAND_DISPLAY"), *flag = getenv("CLARION_CLIPBOARD_ISOLATED_TEST");
  struct stat info;
  if (!flag || strcmp(flag, "1") || !runtime || !root || !prefix(runtime, root) ||
      stat(runtime, &info) || !S_ISDIR(info.st_mode) || (info.st_mode & 0777) != 0700 || info.st_uid != getuid()) fail("unsafe runtime");
  if (!prefix(display, runtime) || getenv("WAYLAND_SOCKET") || stat(display, &info) || !S_ISSOCK(info.st_mode) || info.st_uid != getuid()) fail("unsafe Wayland socket");
  if (path && !prefix(path, root)) fail("unsafe fixture path");
}
static int64_t now_ms(void) { struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts); return ts.tv_sec * 1000LL + ts.tv_nsec / 1000000; }
static void json_string(const char* value) {
  putchar('"'); for (; *value; ++value) { if (*value == '"' || *value == '\\') putchar('\\'); if ((unsigned char)*value >= 32) putchar(*value); } putchar('"');
}
static void source_send(void* data, struct ext_data_control_source_v1* source, const char* mime, int32_t fd) {
  (void)source;
  struct fixture* f = data;
  if (f->requests) { FILE* log = fopen(f->requests, "a"); if (log) { fprintf(log, "%s\n", mime); fclose(log); } }
  if (strcmp(mime, f->mime)) { close(fd); return; }
  fcntl(fd, F_SETFL, O_NONBLOCK);
  size_t sent = 0; const int64_t deadline = now_ms() + 5000;
  while (sent < f->size && now_ms() < deadline) {
    struct pollfd pollfd = {fd, POLLOUT, 0};
    if (poll(&pollfd, 1, 50) <= 0) continue;
    ssize_t n = write(fd, f->bytes + sent, f->size - sent);
    if (n < 0 && (errno == EINTR || errno == EAGAIN)) continue;
    if (n <= 0) break;
    sent += n;
  }
  close(fd);
}
static void source_cancelled(void* data, struct ext_data_control_source_v1* source) { (void)source; ((struct fixture*)data)->done = 1; }
static const struct ext_data_control_source_v1_listener source_listener = {source_send, source_cancelled};
static void offer_mime(void* data, struct ext_data_control_offer_v1* proxy, const char* mime) {
  (void)proxy; struct offer* offer = data;
  if (offer->count < 128 && strlen(mime) < 256) strcpy(offer->mimes[offer->count++], mime);
}
static const struct ext_data_control_offer_v1_listener offer_listener = {offer_mime};
static void data_offer(void* data, struct ext_data_control_device_v1* device, struct ext_data_control_offer_v1* proxy) {
  (void)device; struct fixture* f = data;
  if (f->offer_count == 64) fail("too many fixture offers");
  struct offer* offer = calloc(1, sizeof(*offer));
  offer->fixture = f; offer->proxy = proxy; f->offers[f->offer_count++] = offer;
  ext_data_control_offer_v1_add_listener(proxy, &offer_listener, offer);
}
static void consume(struct fixture* f, struct ext_data_control_offer_v1* proxy) {
  if (!proxy || f->offer_mode || f->done) return;
  struct offer* offer = NULL;
  for (int i = 0; i < f->offer_count; ++i) if (f->offers[i]->proxy == proxy) offer = f->offers[i];
  int supported = 0;
  if (offer) for (int i = 0; i < offer->count; ++i) if (!strcmp(offer->mimes[i], f->mime)) supported = 1;
  if (!supported) return;
  int fds[2]; if (pipe2(fds, O_CLOEXEC)) fail("pipe failed");
  fcntl(fds[0], F_SETFL, O_NONBLOCK);
  ext_data_control_offer_v1_receive(proxy, f->mime, fds[1]); close(fds[1]); wl_display_flush(f->display);
  unsigned char* bytes = malloc(LIMIT); size_t size = 0; int complete = 0;
  const int64_t deadline = now_ms() + 6000;
  while (now_ms() < deadline && size < LIMIT) {
    struct pollfd pollfd = {fds[0], POLLIN, 0}; if (poll(&pollfd, 1, 50) <= 0) continue;
    ssize_t n = read(fds[0], bytes + size, LIMIT - size);
    if (n == 0) { complete = 1; break; }
    if (n < 0 && (errno == EINTR || errno == EAGAIN)) continue;
    if (n < 0) break;
    size += n;
  }
  close(fds[0]);
  if (!complete) fail("clipboard receive timed out or exceeded limit");
  FILE* output = fopen(f->file, "wb"); if (!output || fwrite(bytes, 1, size, output) != size) fail("cannot write fixture output"); fclose(output); free(bytes);
  printf("{\"bytes\":%zu,\"mimes\":[", size);
  for (int i = 0; i < offer->count; ++i) { if (i) putchar(','); json_string(offer->mimes[i]); }
  puts("]}"); fflush(stdout); f->done = 1;
}
static void selection(void* data, struct ext_data_control_device_v1* device, struct ext_data_control_offer_v1* proxy) {
  (void)device; struct fixture* f = data; if (!f->primary) consume(f, proxy);
}
static void primary(void* data, struct ext_data_control_device_v1* device, struct ext_data_control_offer_v1* proxy) {
  (void)device; struct fixture* f = data; if (f->primary) consume(f, proxy);
}
static void finished(void* data, struct ext_data_control_device_v1* device) { (void)device; struct fixture* f = data; f->failed = f->done = 1; }
static const struct ext_data_control_device_v1_listener device_listener = {data_offer, selection, finished, primary};
static void seat_capabilities(void* data, struct wl_seat* seat, uint32_t caps) { (void)data; (void)seat; (void)caps; }
static void seat_name(void* data, struct wl_seat* seat, const char* name) { (void)data; (void)seat; (void)name; }
static const struct wl_seat_listener seat_listener = {seat_capabilities, seat_name};
static void global(void* data, struct wl_registry* registry, uint32_t name, const char* interface, uint32_t version) {
  struct fixture* f = data;
  if (!strcmp(interface, ext_data_control_manager_v1_interface.name) && !f->manager) {
    f->manager = wl_registry_bind(registry, name, &ext_data_control_manager_v1_interface, 1);
  } else if (!strcmp(interface, wl_seat_interface.name) && !f->seat) {
    f->seat = wl_registry_bind(registry, name, &wl_seat_interface, version < 5 ? version : 5);
    wl_seat_add_listener(f->seat, &seat_listener, f);
  }
}
static void removed(void* data, struct wl_registry* registry, uint32_t name) { (void)data; (void)registry; (void)name; }
static const struct wl_registry_listener registry_listener = {global, removed};
int main(int argc, char** argv) {
  struct fixture f = {0};
  if (argc < 2 || (strcmp(argv[1], "offer") && strcmp(argv[1], "get"))) fail("expected offer/get");
  f.offer_mode = !strcmp(argv[1], "offer");
  for (int i = 2; i < argc; ++i) {
    if (!strcmp(argv[i], "--primary")) f.primary = 1;
    else if (!strcmp(argv[i], "--sensitive")) f.sensitive = 1;
    else if (!strcmp(argv[i], "--screenshot")) f.screenshot = 1;
    else if (!strcmp(argv[i], "--mime") && i + 1 < argc) f.mime = argv[++i];
    else if (!strcmp(argv[i], "--file") && i + 1 < argc) f.file = argv[++i];
    else if (!strcmp(argv[i], "--ready") && i + 1 < argc) f.ready = argv[++i];
    else if (!strcmp(argv[i], "--requests") && i + 1 < argc) f.requests = argv[++i];
    else fail("unexpected fixture argument");
  }
  if (!f.mime || !f.file || strlen(f.mime) > 255) fail("missing MIME/file");
  guard(f.file); if (f.ready) guard(f.ready); if (f.requests) guard(f.requests);
  signal(SIGPIPE, SIG_IGN);
  f.display = wl_display_connect(NULL); if (!f.display) fail("cannot connect private Wayland display");
  f.registry = wl_display_get_registry(f.display); wl_registry_add_listener(f.registry, &registry_listener, &f);
  if (wl_display_roundtrip(f.display) < 0 || !f.manager || !f.seat) fail("ext-data-control unavailable");
  f.device = ext_data_control_manager_v1_get_data_device(f.manager, f.seat);
  ext_data_control_device_v1_add_listener(f.device, &device_listener, &f);
  if (f.offer_mode) {
    struct stat info; if (stat(f.file, &info) || info.st_size < 0 || info.st_size > LIMIT) fail("fixture payload size invalid");
    f.size = info.st_size; f.bytes = malloc(f.size + 1);
    FILE* file = fopen(f.file, "rb"); if (!file || fread(f.bytes, 1, f.size, file) != f.size) fail("cannot read fixture payload"); fclose(file);
    f.source = ext_data_control_manager_v1_create_data_source(f.manager);
    ext_data_control_source_v1_add_listener(f.source, &source_listener, &f);
    ext_data_control_source_v1_offer(f.source, f.mime);
    if (f.sensitive) ext_data_control_source_v1_offer(f.source, "x-kde-passwordManagerHint");
    if (f.screenshot) ext_data_control_source_v1_offer(f.source, "application/x-clarion-screenshot");
    if (f.primary) ext_data_control_device_v1_set_primary_selection(f.device, f.source);
    else ext_data_control_device_v1_set_selection(f.device, f.source);
    if (wl_display_roundtrip(f.display) < 0) fail("private selection setup failed");
    if (f.ready) { int fd = open(f.ready, O_CREAT | O_EXCL | O_WRONLY | O_CLOEXEC, 0600); if (fd < 0 || write(fd, "ready\n", 6) != 6) fail("cannot create readiness marker"); close(fd); }
  }
  const int64_t deadline = now_ms() + (f.offer_mode ? 180000 : 10000);
  while (!f.done && now_ms() < deadline) {
    if (wl_display_dispatch_pending(f.display) < 0 || wl_display_flush(f.display) < 0) { f.failed = 1; break; }
    if (f.done) break;
    struct pollfd pollfd = {wl_display_get_fd(f.display), POLLIN, 0};
    if (poll(&pollfd, 1, 100) > 0 && wl_display_dispatch(f.display) < 0) { f.failed = 1; break; }
  }
  if (!f.done && !f.offer_mode) f.failed = 1;
  if (f.source) ext_data_control_source_v1_destroy(f.source);
  for (int i = 0; i < f.offer_count; ++i) { ext_data_control_offer_v1_destroy(f.offers[i]->proxy); free(f.offers[i]); }
  ext_data_control_device_v1_destroy(f.device); ext_data_control_manager_v1_destroy(f.manager);
  if (wl_proxy_get_version((struct wl_proxy*)f.seat) >= WL_SEAT_RELEASE_SINCE_VERSION) wl_seat_release(f.seat); else wl_seat_destroy(f.seat);
  wl_registry_destroy(f.registry); wl_display_flush(f.display); wl_display_disconnect(f.display); free(f.bytes);
  return f.failed ? 1 : 0;
}
