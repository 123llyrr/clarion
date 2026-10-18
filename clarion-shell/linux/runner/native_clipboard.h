// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_NATIVE_CLIPBOARD_H_
#define CLARION_SHELL_NATIVE_CLIPBOARD_H_

#include <flutter_linux/flutter_linux.h>
#include <gtk/gtk.h>
#include <memory>

// One bridge per Flutter engine. Creating it only registers channels; Dart must
// explicitly call start before any real clipboard is observed. Preview engines
// must pass allow_clipboard=false. GDK owns the Wayland connection and dispatch.
//
// MethodChannel clarion/clipboard:
//   start/reconnect -> asynchronously resolved actual availability (bool)
//   status -> {available, monitoring, paused, protocol, generation, error?}
//   setPaused {paused: bool} / stop -> bool
//   publish {mime, data: Uint8List, isScreenshot?: bool} -> bool
// EventChannel clarion/clipboard-events:
//   selection {generation, mime: text/plain|image/png, data, isScreenshot}
//   status {...}, skipped {generation, reason}, cleared {generation}
// Pausing never drops selection ownership and never captures the paused
// selection on resume. Stop unsubscribes while retaining any owned selection;
// destruction finally releases all protocol objects and cancels transfers.
class NativeClipboardBridge final {
 public:
  NativeClipboardBridge(FlBinaryMessenger* messenger, GdkDisplay* display,
                        bool allow_clipboard);
  ~NativeClipboardBridge();
  NativeClipboardBridge(const NativeClipboardBridge&) = delete;
  NativeClipboardBridge& operator=(const NativeClipboardBridge&) = delete;

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

#endif  // CLARION_SHELL_NATIVE_CLIPBOARD_H_
