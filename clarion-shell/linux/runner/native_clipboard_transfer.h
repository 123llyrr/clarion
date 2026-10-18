// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_NATIVE_CLIPBOARD_TRANSFER_H_
#define CLARION_SHELL_NATIVE_CLIPBOARD_TRANSFER_H_

#include <gio/gio.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

namespace clarion_shell {
constexpr size_t kClipboardTextLimit = 1024 * 1024;
constexpr size_t kClipboardPngLimit = 16 * 1024 * 1024;
constexpr gint64 kClipboardTransferUsec = 5 * G_USEC_PER_SEC;

inline std::string ClipboardMimeLower(const std::string& mime) {
  std::string result;
  result.reserve(mime.size());
  for (unsigned char character : mime) {
    if (character != ' ' && character != '\t') {
      result.push_back(g_ascii_tolower(character));
    }
  }
  return result;
}

inline bool ClipboardSensitiveMime(const std::string& mime) {
  const std::string lower = ClipboardMimeLower(mime);
  for (const char* hint : {"password", "secret", "sensitive", "credential",
                           "keepass", "1password", "klipper-ignore",
                           "clipboard-manager-ignore"}) {
    if (lower.find(hint) != std::string::npos) return true;
  }
  return false;
}

inline bool ClipboardTextMime(const std::string& mime) {
  const std::string lower = ClipboardMimeLower(mime);
  return lower == "text/plain" || lower == "text/plain;charset=utf-8" ||
         lower == "utf8_string";
}

inline bool ClipboardPngMime(const std::string& mime) {
  return ClipboardMimeLower(mime) == "image/png";
}

inline bool ClipboardValidPayload(const std::string& mime,
                                  const std::vector<uint8_t>& data) {
  if (mime == "text/plain") {
    return data.size() <= kClipboardTextLimit &&
        (data.empty() || g_utf8_validate(reinterpret_cast<const char*>(data.data()),
                                        data.size(), nullptr));
  }
  static const uint8_t signature[] = {137, 80, 78, 71, 13, 10, 26, 10};
  if (mime != "image/png" || data.size() < 33 || data.size() > kClipboardPngLimit ||
      std::memcmp(data.data(), signature, sizeof(signature)) != 0 ||
      std::memcmp(data.data() + 12, "IHDR", 4) != 0) return false;
  auto word = [&data](size_t offset) {
    return (uint32_t(data[offset]) << 24) | (uint32_t(data[offset + 1]) << 16) |
           (uint32_t(data[offset + 2]) << 8) | uint32_t(data[offset + 3]);
  };
  // Compressed input size alone does not bound a PNG's decoded allocation.
  const uint32_t width = word(16), height = word(20);
  return word(8) == 13 && width > 0 && height > 0 && width <= 16384 &&
         height <= 16384 && uint64_t(width) * height <= 64u * 1024u * 1024u;
}

inline bool ClipboardWaitFd(int fd, short requested, gint64 deadline,
                             GCancellable* cancelled, std::string* error) {
  while (!g_cancellable_is_cancelled(cancelled)) {
    if (g_get_monotonic_time() >= deadline) { *error = "transfer-timeout"; return false; }
    pollfd descriptor = {fd, requested, 0};
    const int result = poll(&descriptor, 1, 50);
    if (result < 0 && errno == EINTR) continue;
    if (result < 0) { *error = "transfer-failed"; return false; }
    if (result == 0) continue;
    if (descriptor.revents & POLLNVAL) { *error = "transfer-closed"; return false; }
    if (descriptor.revents & (requested | POLLHUP | POLLERR)) return true;
  }
  *error = "cancelled";
  return false;
}

// Worker-only functions. Callers own/close the fd; it must be nonblocking.
inline bool ClipboardRead(int fd, size_t limit, GCancellable* cancelled,
                          std::vector<uint8_t>* bytes, std::string* error) {
  const gint64 deadline = g_get_monotonic_time() + kClipboardTransferUsec;
  uint8_t chunk[65536];
  while (ClipboardWaitFd(fd, POLLIN, deadline, cancelled, error)) {
    const size_t amount = std::min(sizeof(chunk), limit - bytes->size() + 1);
    const ssize_t count = read(fd, chunk, amount);
    if (count == 0) return true;
    if (count < 0 && (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK)) continue;
    if (count < 0) { *error = "transfer-failed"; return false; }
    if (size_t(count) > limit - bytes->size()) { *error = "too-large"; return false; }
    bytes->insert(bytes->end(), chunk, chunk + count);
  }
  return false;
}

inline bool ClipboardWrite(int fd, const std::vector<uint8_t>& bytes,
                           GCancellable* cancelled, std::string* error) {
  // A recipient may close its pipe at any point. Block SIGPIPE only on this
  // worker, consume a newly generated pending signal, and restore its mask.
  sigset_t blocked, original, pending;
  sigemptyset(&blocked); sigaddset(&blocked, SIGPIPE);
  const bool masked = pthread_sigmask(SIG_BLOCK, &blocked, &original) == 0;
  if (!masked) { *error = "transfer-failed"; return false; }
  sigpending(&pending);
  const bool already_pending = sigismember(&pending, SIGPIPE);
  const gint64 deadline = g_get_monotonic_time() + kClipboardTransferUsec;
  size_t offset = 0;
  bool success = true;
  while (offset < bytes.size()) {
    if (!ClipboardWaitFd(fd, POLLOUT, deadline, cancelled, error)) { success = false; break; }
    const ssize_t count = write(fd, bytes.data() + offset,
                                std::min(size_t(65536), bytes.size() - offset));
    if (count < 0 && (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK)) continue;
    if (count <= 0) { *error = "transfer-closed"; success = false; break; }
    offset += count;
  }
  if (!already_pending && !sigismember(&original, SIGPIPE)) {
    const timespec immediate = {0, 0};
    while (sigtimedwait(&blocked, nullptr, &immediate) < 0 && errno == EINTR) {}
  }
  pthread_sigmask(SIG_SETMASK, &original, nullptr);
  return success;
}
}  // namespace clarion_shell
#endif  // CLARION_SHELL_NATIVE_CLIPBOARD_TRANSFER_H_
