// SPDX-License-Identifier: GPL-3.0-or-later
#include "../../linux/runner/native_clipboard_transfer.h"

#include <fcntl.h>
#include <cassert>
#include <chrono>
#include <thread>

namespace {
void Pipe(int (&fds)[2]) { assert(pipe2(fds, O_CLOEXEC | O_NONBLOCK) == 0); }
void Word(std::vector<uint8_t>* png, size_t offset, uint32_t value) {
  for (int shift = 24; shift >= 0; shift -= 8) (*png)[offset++] = (value >> shift) & 255;
}
}  // namespace

int main() {
  using namespace clarion_shell;
  assert(ClipboardTextMime("text/plain; charset=UTF-8"));
  assert(ClipboardTextMime("UTF8_STRING"));
  assert(!ClipboardTextMime("text/html"));
  assert(ClipboardSensitiveMime("x-kde-passwordManagerHint"));
  assert(ClipboardSensitiveMime("application/x-keepassxc"));
  assert(ClipboardSensitiveMime("application/x-clipboard-SENSITIVE"));
  assert(!ClipboardSensitiveMime("text/plain"));
  assert(ClipboardValidPayload("text/plain", {'h', 'i'}));
  assert(!ClipboardValidPayload("text/plain", {0xc0, 0x80}));
  assert(!ClipboardValidPayload("text/plain", std::vector<uint8_t>(kClipboardTextLimit + 1, 'a')));
  std::vector<uint8_t> png(33);
  const uint8_t signature[] = {137, 80, 78, 71, 13, 10, 26, 10};
  std::copy(signature, signature + 8, png.begin());
  Word(&png, 8, 13); std::memcpy(png.data() + 12, "IHDR", 4);
  Word(&png, 16, 3840); Word(&png, 20, 2400);
  assert(ClipboardValidPayload("image/png", png));
  Word(&png, 16, 65535);
  assert(!ClipboardValidPayload("image/png", png));
  Word(&png, 16, 16384); Word(&png, 20, 16384);
  assert(!ClipboardValidPayload("image/png", png));

  g_autoptr(GCancellable) cancelled = g_cancellable_new();
  {
    int fds[2]; Pipe(fds);
    std::vector<uint8_t> expected(400000, 'x'), actual;
    std::thread writer([&]() {
      std::string error;
      assert(ClipboardWrite(fds[1], expected, cancelled, &error));
      close(fds[1]);
    });
    std::string error;
    assert(ClipboardRead(fds[0], expected.size(), cancelled, &actual, &error));
    assert(actual == expected);
    close(fds[0]); writer.join();
  }
  {
    int fds[2]; Pipe(fds);
    const std::vector<uint8_t> expected(4097, 'x');
    std::string error;
    assert(ClipboardWrite(fds[1], expected, cancelled, &error));
    close(fds[1]);
    std::vector<uint8_t> actual;
    assert(!ClipboardRead(fds[0], 4096, cancelled, &actual, &error));
    assert(error == "too-large");
    close(fds[0]);
  }
  {
    int fds[2]; Pipe(fds); close(fds[0]);
    std::string error;
    assert(!ClipboardWrite(fds[1], {'a'}, cancelled, &error));
    assert(error == "transfer-closed");
    close(fds[1]);  // Reaching here establishes that SIGPIPE did not kill us.
  }
  {
    int fds[2]; Pipe(fds);
    std::thread cancel([&]() { std::this_thread::sleep_for(std::chrono::milliseconds(20)); g_cancellable_cancel(cancelled); });
    const auto started = std::chrono::steady_clock::now();
    std::vector<uint8_t> actual;
    std::string error;
    assert(!ClipboardRead(fds[0], 4096, cancelled, &actual, &error));
    assert(error == "cancelled");
    assert(std::chrono::steady_clock::now() - started < std::chrono::seconds(1));
    cancel.join(); close(fds[0]); close(fds[1]);
  }
  return 0;
}
