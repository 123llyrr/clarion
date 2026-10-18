// SPDX-License-Identifier: GPL-3.0-or-later
#include "native_clipboard.h"

#include <fcntl.h>
#include <gdk/gdkwayland.h>
#include <wayland-client.h>

#include <atomic>
#include <cstring>
#include <memory>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

#include "ext-data-control-v1-client-protocol.h"
#include "native_clipboard_transfer.h"

namespace {
constexpr char kScreenshotMime[] = "application/x-clarion-screenshot";
constexpr size_t kMaxOfferMimes = 128;
constexpr int kMaxConcurrentWrites = 8;

void Respond(FlMethodCall* call, FlMethodResponse* response) {
  g_autoptr(GError) error = nullptr;
  if (!fl_method_call_respond(call, response, &error)) {
    g_warning("Unable to respond to clipboard method: %s", error->message);
  }
}

void Success(FlMethodCall* call, FlValue* value) {
  g_autoptr(FlMethodResponse) response = FL_METHOD_RESPONSE(fl_method_success_response_new(value));
  Respond(call, response);
}

void Error(FlMethodCall* call, const char* code, const char* message) {
  g_autoptr(FlMethodResponse) response = FL_METHOD_RESPONSE(fl_method_error_response_new(code, message, nullptr));
  Respond(call, response);
}
}  // namespace

struct NativeClipboardBridge::Impl {
  struct Life {
    Impl* owner = nullptr;  // Main-context access only; workers never dereference.
    GCancellable* writes = g_cancellable_new();
    std::atomic<int> active_writes{0};
    ~Life() { g_object_unref(writes); }
  };
  struct Offer {
    Impl* owner;
    ext_data_control_offer_v1* proxy;
    std::vector<std::string> mimes;
    bool sensitive = false;
    bool overflow = false;
    bool screenshot = false;
    bool own = false;
  };
  struct OwnedSource {
    Impl* owner;
    ext_data_control_source_v1* proxy;
    std::string mime;
    std::string marker;
    std::shared_ptr<const std::vector<uint8_t>> bytes;
  };
  struct ReadTask {
    int fd;
    uint64_t generation;
    std::string mime;
    bool screenshot;
    std::vector<uint8_t> bytes;
    std::string error;
    ~ReadTask() { close(fd); }
  };
  struct WriteTask {
    int fd;
    std::shared_ptr<Life> life;
    std::shared_ptr<const std::vector<uint8_t>> bytes;
    ~WriteTask() { close(fd); --life->active_writes; }
  };

  GdkDisplay* gdk_display = nullptr;
  wl_display* display = nullptr;  // Borrowed; GDK dispatches its default queue.
  wl_registry* registry = nullptr;
  wl_callback* discovery = nullptr;
  wl_seat* seat = nullptr;
  ext_data_control_manager_v1* manager = nullptr;
  ext_data_control_device_v1* device = nullptr;
  uint32_t manager_name = 0, seat_name = 0;
  FlMethodChannel* methods = nullptr;
  FlEventChannel* events = nullptr;
  bool allowed;
  bool enabled = false;
  bool paused = false;
  bool listening = false;
  std::string status_error = "stopped";
  std::shared_ptr<Life> life = std::make_shared<Life>();
  uint64_t generation = 0;
  GCancellable* reading = nullptr;
  ext_data_control_offer_v1* current_offer = nullptr;
  std::vector<FlMethodCall*> starts;
  guint start_timeout = 0;
  std::unordered_map<ext_data_control_offer_v1*, std::unique_ptr<Offer>> offers;
  std::unordered_map<ext_data_control_source_v1*, std::unique_ptr<OwnedSource>> owned;

  Impl(FlBinaryMessenger* messenger, GdkDisplay* input_display, bool allow)
      : gdk_display(GDK_DISPLAY(g_object_ref(input_display))), allowed(allow) {
    life->owner = this;
    g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
    methods = fl_method_channel_new(messenger, "clarion/clipboard", FL_METHOD_CODEC(codec));
    fl_method_channel_set_method_call_handler(methods, Method, this, nullptr);
    events = fl_event_channel_new(messenger, "clarion/clipboard-events", FL_METHOD_CODEC(codec));
    fl_event_channel_set_stream_handlers(events, Listen, Cancel, this, nullptr);
  }

  ~Impl() {
    listening = false;
    fl_method_channel_set_method_call_handler(methods, nullptr, nullptr, nullptr);
    fl_event_channel_set_stream_handlers(events, nullptr, nullptr, nullptr, nullptr);
    life->owner = nullptr;
    g_cancellable_cancel(life->writes);
    Stop("stopped");
    for (auto& item : owned) ext_data_control_source_v1_destroy(item.first);
    owned.clear();
    if (display != nullptr) wl_display_flush(display);
    g_clear_object(&methods);
    g_clear_object(&events);
    g_clear_object(&gdk_display);
  }

  bool Available() const { return enabled && device != nullptr; }
  FlValue* Status() const {
    FlValue* value = fl_value_new_map();
    fl_value_set_string_take(value, "type", fl_value_new_string("status"));
    fl_value_set_string_take(value, "available", fl_value_new_bool(Available()));
    fl_value_set_string_take(value, "monitoring", fl_value_new_bool(enabled));
    fl_value_set_string_take(value, "paused", fl_value_new_bool(paused));
    fl_value_set_string_take(value, "protocol", fl_value_new_string("ext-data-control-v1"));
    fl_value_set_string_take(value, "generation", fl_value_new_int(generation));
    if (!status_error.empty()) fl_value_set_string_take(value, "error", fl_value_new_string(status_error.c_str()));
    return value;
  }
  void Emit(FlValue* event) {
    if (!listening) return;
    g_autoptr(GError) error = nullptr;
    fl_event_channel_send(events, event, nullptr, &error);
    // A Dart hot restart may cancel the stream before the native notification.
    // Clipboard contents and offered MIME names are deliberately never logged.
  }
  void EmitStatus() { g_autoptr(FlValue) value = Status(); Emit(value); }
  void Skipped(const char* reason) {
    g_autoptr(FlValue) value = fl_value_new_map();
    fl_value_set_string_take(value, "type", fl_value_new_string("skipped"));
    fl_value_set_string_take(value, "generation", fl_value_new_int(generation));
    fl_value_set_string_take(value, "reason", fl_value_new_string(reason));
    Emit(value);
  }
  void CancelRead() {
    if (reading != nullptr) g_cancellable_cancel(reading);
    g_clear_object(&reading);
  }
  void CompleteStarts(bool available) {
    if (start_timeout != 0) g_source_remove(start_timeout);
    start_timeout = 0;
    auto pending = std::move(starts);
    starts.clear();
    for (auto* call : pending) {
      g_autoptr(FlValue) result = fl_value_new_bool(available);
      Success(call, result);
      g_object_unref(call);
    }
  }
  void AwaitStart(FlMethodCall* call) {
    if (Available() || !enabled || discovery == nullptr) {
      g_autoptr(FlValue) result = fl_value_new_bool(Available());
      Success(call, result);
      return;
    }
    starts.push_back(FL_METHOD_CALL(g_object_ref(call)));
    if (start_timeout == 0) {
      start_timeout = g_timeout_add(3000, [](gpointer data) -> gboolean {
        auto* self = static_cast<Impl*>(data);
        self->start_timeout = 0;
        self->status_error = "discovery-timeout";
        self->CompleteStarts(false);
        self->EmitStatus();
        return G_SOURCE_REMOVE;
      }, this);
    }
  }
  void DestroyOffer(ext_data_control_offer_v1* proxy) {
    if (proxy == nullptr) return;
    auto found = offers.find(proxy);
    if (found != offers.end()) {
      ext_data_control_offer_v1_destroy(proxy);
      offers.erase(found);
    }
    if (current_offer == proxy) current_offer = nullptr;
  }
  void ReleaseDevice() {
    ++generation;
    CancelRead();
    for (auto& item : offers) ext_data_control_offer_v1_destroy(item.first);
    offers.clear(); current_offer = nullptr;
    if (device != nullptr) ext_data_control_device_v1_destroy(device);
    device = nullptr;
  }
  void ReleaseSeat() {
    if (seat == nullptr) return;
    if (wl_proxy_get_version(reinterpret_cast<wl_proxy*>(seat)) >= WL_SEAT_RELEASE_SINCE_VERSION) {
      wl_seat_release(seat);
    } else {
      wl_seat_destroy(seat);
    }
    seat = nullptr; seat_name = 0;
  }
  void Stop(const char* reason) {
    enabled = false;
    CompleteStarts(false);
    ReleaseDevice();
    if (discovery != nullptr) wl_callback_destroy(discovery);
    discovery = nullptr;
    if (manager != nullptr) ext_data_control_manager_v1_destroy(manager);
    manager = nullptr; manager_name = 0;
    ReleaseSeat();
    if (registry != nullptr) wl_registry_destroy(registry);
    registry = nullptr;
    status_error = reason;
    if (display != nullptr) wl_display_flush(display);
    // Owned sources deliberately survive monitoring being disabled: turning
    // history off must not erase a copied entry which another app may paste.
    EmitStatus();
  }
  bool Start() {
    if (!allowed) { status_error = "preview-disabled"; EmitStatus(); return false; }
    if (!GDK_IS_WAYLAND_DISPLAY(gdk_display)) { status_error = "wayland-required"; EmitStatus(); return false; }
    if (enabled) { EmitStatus(); return true; }
    display = gdk_wayland_display_get_wl_display(gdk_display);
    if (display == nullptr || wl_display_get_error(display) != 0) {
      status_error = "wayland-disconnected"; EmitStatus(); return false;
    }
    enabled = true; status_error = "connecting";
    registry = wl_display_get_registry(display);
    static const wl_registry_listener registry_listener = {Global, GlobalRemoved};
    wl_registry_add_listener(registry, &registry_listener, this);
    discovery = wl_display_sync(display);
    static const wl_callback_listener callback_listener = {Discovered};
    wl_callback_add_listener(discovery, &callback_listener, this);
    wl_display_flush(display);
    EmitStatus();
    return true;  // Monitoring accepted; availability arrives asynchronously.
  }
  void CreateDevice() {
    if (!enabled || manager == nullptr || seat == nullptr || device != nullptr) return;
    device = ext_data_control_manager_v1_get_data_device(manager, seat);
    static const ext_data_control_device_v1_listener listener = {DataOffer, Selection, Finished, Primary};
    ext_data_control_device_v1_add_listener(device, &listener, this);
    status_error.clear();
    CompleteStarts(true);
    EmitStatus();
    wl_display_flush(display);
  }

  static void Global(void* data, wl_registry* registry_proxy, uint32_t name,
                     const char* interface, uint32_t version) {
    auto* self = static_cast<Impl*>(data);
    if (std::strcmp(interface, ext_data_control_manager_v1_interface.name) == 0 && self->manager == nullptr) {
      self->manager_name = name;
      self->manager = static_cast<ext_data_control_manager_v1*>(wl_registry_bind(registry_proxy, name, &ext_data_control_manager_v1_interface, 1));
    } else if (std::strcmp(interface, wl_seat_interface.name) == 0 && self->seat == nullptr) {
      self->seat_name = name;
      self->seat = static_cast<wl_seat*>(wl_registry_bind(registry_proxy, name, &wl_seat_interface, std::min(version, 5u)));
      static const wl_seat_listener listener = {SeatCapabilities, SeatName};
      wl_seat_add_listener(self->seat, &listener, self);
    }
    self->CreateDevice();
  }
  static void GlobalRemoved(void* data, wl_registry*, uint32_t name) {
    auto* self = static_cast<Impl*>(data);
    if (name != self->manager_name && name != self->seat_name) return;
    self->ReleaseDevice();
    if (name == self->manager_name) {
      ext_data_control_manager_v1_destroy(self->manager);
      self->manager = nullptr; self->manager_name = 0;
    }
    if (name == self->seat_name) {
      self->ReleaseSeat();
    }
    self->status_error = "protocol-unavailable";
    self->EmitStatus();
  }
  static void Discovered(void* data, wl_callback* callback, uint32_t) {
    auto* self = static_cast<Impl*>(data);
    wl_callback_destroy(callback); self->discovery = nullptr;
    self->CreateDevice();
    if (self->device == nullptr) {
      self->status_error = self->manager == nullptr ? "data-control-unavailable" : "seat-unavailable";
      self->CompleteStarts(false);
      self->EmitStatus();
    }
  }
  static void SeatCapabilities(void*, wl_seat*, uint32_t) {}
  static void SeatName(void*, wl_seat*, const char*) {}
  static void DataOffer(void* data, ext_data_control_device_v1*, ext_data_control_offer_v1* proxy) {
    auto* self = static_cast<Impl*>(data);
    // A well-behaved server immediately associates each offer with clipboard
    // or primary. Bound unassociated objects defensively before accepting more.
    if (self->offers.size() > 8) {
      ext_data_control_offer_v1_destroy(proxy);
      return;
    }
    auto offer = std::make_unique<Offer>();
    offer->owner = self; offer->proxy = proxy;
    static const ext_data_control_offer_v1_listener listener = {OfferMime};
    ext_data_control_offer_v1_add_listener(proxy, &listener, offer.get());
    self->offers.emplace(proxy, std::move(offer));
  }
  static void OfferMime(void* data, ext_data_control_offer_v1*, const char* mime) {
    auto* offer = static_cast<Offer*>(data);
    const size_t length = std::strlen(mime);
    if (length > 256) { offer->overflow = true; return; }
    const std::string offered(mime, length);
    offer->sensitive |= clarion_shell::ClipboardSensitiveMime(offered);
    offer->screenshot |= offered == kScreenshotMime;
    for (const auto& source : offer->owner->owned) {
      if (offered == source.second->marker) offer->own = true;
    }
    if (offer->mimes.size() >= kMaxOfferMimes) { offer->overflow = true; return; }
    offer->mimes.push_back(offered);
  }
  static void Selection(void* data, ext_data_control_device_v1*, ext_data_control_offer_v1* proxy) {
    auto* self = static_cast<Impl*>(data);
    ++self->generation;
    self->CancelRead();
    if (self->current_offer != proxy) self->DestroyOffer(self->current_offer);
    self->current_offer = proxy;
    if (proxy == nullptr) {
      if (self->paused) return;
      g_autoptr(FlValue) value = fl_value_new_map();
      fl_value_set_string_take(value, "type", fl_value_new_string("cleared"));
      fl_value_set_string_take(value, "generation", fl_value_new_int(self->generation));
      self->Emit(value);
      return;
    }
    auto found = self->offers.find(proxy);
    if (found == self->offers.end()) { self->Skipped("invalid-offer"); return; }
    const Offer& offer = *found->second;
    if (offer.sensitive || offer.overflow) {
      if (self->paused) return;
      self->Skipped(offer.sensitive ? "sensitive" : "unsupported-offer"); return;
    }
    if (offer.own || self->paused || !self->listening || !self->enabled) return;
    std::string chosen, canonical;
    for (const std::string& mime : offer.mimes) {
      if (clarion_shell::ClipboardPngMime(mime)) { chosen = mime; canonical = "image/png"; break; }
    }
    if (chosen.empty()) {
      for (const std::string& mime : offer.mimes) {
        if (clarion_shell::ClipboardTextMime(mime)) {
          chosen = mime; canonical = "text/plain";
          if (clarion_shell::ClipboardMimeLower(mime) == "text/plain;charset=utf-8") break;
        }
      }
    }
    if (chosen.empty()) { self->Skipped("unsupported-mime"); return; }
    int descriptors[2];
    if (pipe2(descriptors, O_CLOEXEC) != 0) { self->Skipped("transfer-failed"); return; }
    if (fcntl(descriptors[0], F_SETFL, O_NONBLOCK) < 0) {
      close(descriptors[0]); close(descriptors[1]); self->Skipped("transfer-failed"); return;
    }
    self->reading = g_cancellable_new();
    auto* read = new ReadTask{descriptors[0], self->generation, canonical, offer.screenshot, {}, {}};
    auto* context = new std::shared_ptr<Life>(self->life);
    GTask* task = g_task_new(nullptr, self->reading, ReadFinished, context);
    g_task_set_task_data(task, read, [](gpointer pointer) { delete static_cast<ReadTask*>(pointer); });
    g_task_run_in_thread(task, ReadWorker);
    g_object_unref(task);
    ext_data_control_offer_v1_receive(proxy, chosen.c_str(), descriptors[1]);
    close(descriptors[1]);
    wl_display_flush(self->display);
  }
  static void Primary(void* data, ext_data_control_device_v1*, ext_data_control_offer_v1* proxy) {
    // The protocol announces primary alongside clipboard. Discard its offer
    // without requesting any bytes or emitting any history event.
    static_cast<Impl*>(data)->DestroyOffer(proxy);
  }
  static void Finished(void* data, ext_data_control_device_v1*) {
    auto* self = static_cast<Impl*>(data);
    self->Stop("data-control-finished");
  }
  static void ReadWorker(GTask* task, gpointer, gpointer data, GCancellable* cancelled) {
    auto* read = static_cast<ReadTask*>(data);
    const size_t limit = read->mime == "image/png" ? clarion_shell::kClipboardPngLimit : clarion_shell::kClipboardTextLimit;
    bool valid = clarion_shell::ClipboardRead(read->fd, limit, cancelled, &read->bytes, &read->error);
    if (valid && !clarion_shell::ClipboardValidPayload(read->mime, read->bytes)) {
      read->error = "invalid-content"; valid = false;
    }
    g_task_return_boolean(task, valid);
  }
  static void ReadFinished(GObject*, GAsyncResult* result, gpointer data) {
    std::unique_ptr<std::shared_ptr<Life>> context(static_cast<std::shared_ptr<Life>*>(data));
    Impl* self = (*context)->owner;
    auto* task = G_TASK(result);
    auto* read = static_cast<ReadTask*>(g_task_get_task_data(task));
    g_autoptr(GError) error = nullptr;
    const bool valid = g_task_propagate_boolean(task, &error);
    if (self == nullptr || !self->enabled || self->paused || read->generation != self->generation) return;
    g_clear_object(&self->reading);
    if (!valid) { if (read->error != "cancelled") self->Skipped(read->error.empty() ? "transfer-failed" : read->error.c_str()); return; }
    g_autoptr(FlValue) value = fl_value_new_map();
    fl_value_set_string_take(value, "type", fl_value_new_string("selection"));
    fl_value_set_string_take(value, "generation", fl_value_new_int(read->generation));
    fl_value_set_string_take(value, "mime", fl_value_new_string(read->mime.c_str()));
    fl_value_set_string_take(value, "isScreenshot", fl_value_new_bool(read->screenshot));
    g_autoptr(GBytes) bytes = g_bytes_new(read->bytes.data(), read->bytes.size());
    fl_value_set_string_take(value, "data", fl_value_new_uint8_list_from_bytes(bytes));
    self->Emit(value);
  }

  bool Publish(const std::string& mime, std::vector<uint8_t> bytes, bool screenshot) {
    if (!Available() || owned.size() >= 8) return false;
    ++generation;
    CancelRead();
    auto source = std::make_unique<OwnedSource>();
    source->owner = this;
    source->proxy = ext_data_control_manager_v1_create_data_source(manager);
    source->mime = mime;
    g_autofree gchar* token = g_uuid_string_random();
    source->marker = std::string("application/x-clarion-clipboard-") + token;
    source->bytes = std::make_shared<const std::vector<uint8_t>>(std::move(bytes));
    static const ext_data_control_source_v1_listener listener = {SourceSend, SourceCancelled};
    ext_data_control_source_v1_add_listener(source->proxy, &listener, source.get());
    ext_data_control_source_v1_offer(source->proxy, mime.c_str());
    if (mime == "text/plain") {
      ext_data_control_source_v1_offer(source->proxy, "text/plain;charset=utf-8");
      ext_data_control_source_v1_offer(source->proxy, "UTF8_STRING");
    }
    ext_data_control_source_v1_offer(source->proxy, source->marker.c_str());
    if (screenshot) ext_data_control_source_v1_offer(source->proxy, kScreenshotMime);
    auto* proxy = source->proxy;
    owned.emplace(proxy, std::move(source));
    ext_data_control_device_v1_set_selection(device, proxy);
    wl_display_flush(display);
    return true;
  }
  static void SourceSend(void* data, ext_data_control_source_v1*, const char* mime, int32_t fd) {
    auto* source = static_cast<OwnedSource*>(data);
    const bool supported = source->mime == "text/plain" ? clarion_shell::ClipboardTextMime(mime) : clarion_shell::ClipboardPngMime(mime);
    if (!supported || fcntl(fd, F_SETFL, O_NONBLOCK) < 0) { close(fd); return; }
    auto life = source->owner->life;
    if (life->active_writes.fetch_add(1) >= kMaxConcurrentWrites) { --life->active_writes; close(fd); return; }
    auto* write = new WriteTask{fd, life, source->bytes};
    GTask* task = g_task_new(nullptr, life->writes, nullptr, nullptr);
    g_task_set_task_data(task, write, [](gpointer pointer) { delete static_cast<WriteTask*>(pointer); });
    g_task_run_in_thread(task, [](GTask* task, gpointer, gpointer data, GCancellable* cancelled) {
      auto* write = static_cast<WriteTask*>(data);
      std::string error;
      g_task_return_boolean(task, clarion_shell::ClipboardWrite(write->fd, *write->bytes, cancelled, &error));
    });
    g_object_unref(task);
  }
  static void SourceCancelled(void* data, ext_data_control_source_v1* proxy) {
    auto* source = static_cast<OwnedSource*>(data);
    Impl* self = source->owner;
    ext_data_control_source_v1_destroy(proxy);
    self->owned.erase(proxy);
  }
  static void Method(FlMethodChannel*, FlMethodCall* call, gpointer data) {
    auto* self = static_cast<Impl*>(data);
    const char* name = fl_method_call_get_name(call);
    if (std::strcmp(name, "start") == 0 || std::strcmp(name, "reconnect") == 0) {
      if (std::strcmp(name, "reconnect") == 0) self->Stop("reconnecting");
      self->Start(); self->AwaitStart(call);
    } else if (std::strcmp(name, "stop") == 0) {
      self->Stop("stopped");
      g_autoptr(FlValue) result = fl_value_new_bool(true); Success(call, result);
    } else if (std::strcmp(name, "status") == 0) {
      g_autoptr(FlValue) result = self->Status(); Success(call, result);
    } else if (std::strcmp(name, "setPaused") == 0) {
      FlValue* arguments = fl_method_call_get_args(call);
      FlValue* paused = arguments != nullptr && fl_value_get_type(arguments) == FL_VALUE_TYPE_MAP ? fl_value_lookup_string(arguments, "paused") : nullptr;
      if (paused == nullptr || fl_value_get_type(paused) != FL_VALUE_TYPE_BOOL) {
        Error(call, "invalid-argument", "setPaused requires a paused boolean"); return;
      }
      if (self->paused != bool(fl_value_get_bool(paused))) {
        self->paused = fl_value_get_bool(paused);
        ++self->generation;
        self->CancelRead();
      }
      self->EmitStatus();
      g_autoptr(FlValue) result = fl_value_new_bool(true); Success(call, result);
    } else if (std::strcmp(name, "publish") == 0) {
      if (!self->Available()) { Error(call, "unavailable", "Wayland clipboard data-control is unavailable"); return; }
      FlValue* arguments = fl_method_call_get_args(call);
      if (arguments == nullptr || fl_value_get_type(arguments) != FL_VALUE_TYPE_MAP) { Error(call, "invalid-argument", "publish requires mime and Uint8List data"); return; }
      FlValue* mime_value = fl_value_lookup_string(arguments, "mime");
      FlValue* payload = fl_value_lookup_string(arguments, "data");
      FlValue* screenshot_value = fl_value_lookup_string(arguments, "isScreenshot");
      if (mime_value == nullptr || fl_value_get_type(mime_value) != FL_VALUE_TYPE_STRING || payload == nullptr || fl_value_get_type(payload) != FL_VALUE_TYPE_UINT8_LIST ||
          (screenshot_value != nullptr && fl_value_get_type(screenshot_value) != FL_VALUE_TYPE_BOOL)) {
        Error(call, "invalid-argument", "publish requires a supported MIME and Uint8List data"); return;
      }
      const std::string offered = fl_value_get_string(mime_value);
      const std::string mime = clarion_shell::ClipboardTextMime(offered) ? "text/plain" : clarion_shell::ClipboardPngMime(offered) ? "image/png" : "";
      const size_t size = fl_value_get_length(payload);
      const size_t limit = mime == "text/plain" ? clarion_shell::kClipboardTextLimit : clarion_shell::kClipboardPngLimit;
      const bool screenshot = screenshot_value != nullptr && fl_value_get_bool(screenshot_value);
      if (mime.empty() || size > limit || (screenshot && mime != "image/png")) { Error(call, "invalid-content", "Clipboard content type or size is unsupported"); return; }
      const uint8_t* bytes = fl_value_get_uint8_list(payload);
      std::vector<uint8_t> copied;
      if (size != 0) copied.assign(bytes, bytes + size);
      if (!clarion_shell::ClipboardValidPayload(mime, copied)) { Error(call, "invalid-content", "Clipboard content is not valid UTF-8 or a bounded PNG"); return; }
      g_autoptr(FlValue) result = fl_value_new_bool(self->Publish(mime, std::move(copied), screenshot)); Success(call, result);
    } else {
      g_autoptr(FlMethodResponse) response = FL_METHOD_RESPONSE(fl_method_not_implemented_response_new()); Respond(call, response);
    }
  }
  static FlMethodErrorResponse* Listen(FlEventChannel*, FlValue*, gpointer data) {
    auto* self = static_cast<Impl*>(data); self->listening = true; self->EmitStatus(); return nullptr;
  }
  static FlMethodErrorResponse* Cancel(FlEventChannel*, FlValue*, gpointer data) {
    auto* self = static_cast<Impl*>(data); self->listening = false; self->Stop("stopped"); return nullptr;
  }
};

NativeClipboardBridge::NativeClipboardBridge(FlBinaryMessenger* messenger,
    GdkDisplay* display, bool allow_clipboard)
    : impl_(std::make_unique<Impl>(messenger, display, allow_clipboard)) {}
NativeClipboardBridge::~NativeClipboardBridge() = default;
