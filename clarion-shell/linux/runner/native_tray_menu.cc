// SPDX-License-Identifier: GPL-3.0-or-later
#include "native_tray_menu.h"

#include <gtk-layer-shell.h>
#include <libdbusmenu-glib/client.h>
#include <libdbusmenu-glib/menuitem.h>
#include <libdbusmenu-gtk/client.h>
#include <libdbusmenu-gtk/menu.h>

#include <algorithm>
#include <utility>

namespace {
constexpr size_t kMaxCachedMenus = 8;
constexpr gint64 kInputMaxAgeUs = 2 * G_USEC_PER_SEC;
constexpr gint64 kLayoutTimeoutUs = 1500 * 1000;

bool IsPointerEvent(const GdkEvent* event) {
  return event != nullptr &&
      (event->type == GDK_BUTTON_PRESS || event->type == GDK_BUTTON_RELEASE);
}

bool IsKeyEvent(const GdkEvent* event) {
  return event != nullptr &&
      (event->type == GDK_KEY_PRESS || event->type == GDK_KEY_RELEASE);
}

bool SameInput(const GdkEvent* a, const GdkEvent* b) {
  if (a == nullptr || b == nullptr || a->type != b->type ||
      gdk_event_get_time(a) != gdk_event_get_time(b) ||
      gdk_event_get_device(a) != gdk_event_get_device(b)) return false;
  if (IsPointerEvent(a)) return a->button.button == b->button.button;
  if (IsKeyEvent(a)) return a->key.hardware_keycode == b->key.hardware_keycode;
  return false;
}

bool HasVisibleChildren(GtkWidget* menu) {
  GList* children = gtk_container_get_children(GTK_CONTAINER(menu));
  bool visible = false;
  for (GList* item = children; item != nullptr; item = item->next) {
    if (gtk_widget_get_visible(GTK_WIDGET(item->data))) {
      visible = true;
      break;
    }
  }
  g_list_free(children);
  return visible;
}

void CountVisible(GtkWidget* menu, int* items, int* submenus) {
  if (!gtk_widget_get_mapped(menu)) return;
  GList* children = gtk_container_get_children(GTK_CONTAINER(menu));
  for (GList* item = children; item != nullptr; item = item->next) {
    GtkWidget* widget = GTK_WIDGET(item->data);
    if (!gtk_widget_get_mapped(widget)) continue;
    ++*items;
    if (!GTK_IS_MENU_ITEM(widget)) continue;
    GtkWidget* submenu = gtk_menu_item_get_submenu(GTK_MENU_ITEM(widget));
    if (submenu != nullptr && gtk_widget_get_mapped(submenu)) {
      ++*submenus;
      CountVisible(submenu, items, submenus);
    }
  }
  g_list_free(children);
}

void CollectBounds(GtkWidget* menu, int depth,
                   std::vector<NativeTrayMenu::ItemBounds>* bounds) {
  if (!gtk_widget_get_mapped(menu)) return;
  GtkWidget* toplevel = gtk_widget_get_toplevel(menu);
  GdkWindow* window = gtk_widget_get_window(toplevel);
  int root_x = 0, root_y = 0;
  gdk_window_get_origin(window, &root_x, &root_y);
  GList* children = gtk_container_get_children(GTK_CONTAINER(menu));
  int index = 0;
  for (GList* child = children; child != nullptr; child = child->next, ++index) {
    GtkWidget* item = GTK_WIDGET(child->data);
    if (!gtk_widget_get_mapped(item)) continue;
    int x = 0, y = 0;
    if (!gtk_widget_translate_coordinates(item, toplevel, 0, 0, &x, &y)) continue;
    const gchar* label = GTK_IS_MENU_ITEM(item)
        ? gtk_menu_item_get_label(GTK_MENU_ITEM(item)) : nullptr;
    bounds->push_back({label == nullptr ? "" : label, depth, index,
        root_x + x, root_y + y, gtk_widget_get_allocated_width(item),
        gtk_widget_get_allocated_height(item), gtk_widget_is_sensitive(item) != FALSE});
    GtkWidget* submenu = GTK_IS_MENU_ITEM(item)
        ? gtk_menu_item_get_submenu(GTK_MENU_ITEM(item)) : nullptr;
    if (submenu != nullptr) CollectBounds(submenu, depth + 1, bounds);
  }
  g_list_free(children);
}
}  // namespace

struct NativeTrayMenu::Submenu {
  Entry* entry = nullptr;
  DbusmenuMenuitem* model = nullptr;  // Weak; layout replacement can remove it.
  GtkMenuItem* item = nullptr;  // Weak; do not retain a model/widget cycle.
  bool in_flight = false;
  uint64_t sequence = 0;

  void Detach() {
    entry = nullptr;
    if (item != nullptr) {
      g_signal_handlers_disconnect_by_data(item, this);
      g_object_remove_weak_pointer(G_OBJECT(item), reinterpret_cast<gpointer*>(&item));
      item = nullptr;
    }
    if (model != nullptr) {
      g_object_remove_weak_pointer(G_OBJECT(model), reinterpret_cast<gpointer*>(&model));
      model = nullptr;
    }
  }
  ~Submenu() { Detach(); }
};

struct NativeTrayMenu::AboutRequest {
  std::shared_ptr<Entry*> lifetime;
  std::shared_ptr<Submenu> submenu;
  int64_t request_id;
  uint64_t sequence = 0;
};

struct NativeTrayMenu::Entry {
  NativeTrayMenu* host;
  std::string owner;
  std::string path;
  GtkWidget* menu = nullptr;
  DbusmenuClient* client = nullptr;  // Owned; outlives an attached menu's dispose.
  guint owner_watch = 0;
  uint64_t last_used = 0;
  bool alive = true;
  bool requested_empty_root = false;
  guint reflow_source = 0;
  std::shared_ptr<Entry*> lifetime = std::make_shared<Entry*>(this);
  std::vector<std::shared_ptr<Submenu>> submenus;

  ~Entry() {
    *lifetime = nullptr;
    if (reflow_source != 0) g_source_remove(reflow_source);
    for (const auto& submenu : submenus) submenu->Detach();
    submenus.clear();
    if (owner_watch != 0) g_bus_unwatch_name(owner_watch);
    if (client != nullptr) g_signal_handlers_disconnect_by_data(client, this);
    if (menu != nullptr) {
      g_signal_handlers_disconnect_by_data(menu, this);
      gtk_widget_destroy(menu);
      g_object_unref(menu);
    }
    g_clear_object(&client);
  }
};

struct NativeTrayMenu::ObservedWidget {
  GtkWidget* widget = nullptr;  // Weak; GTK can destroy a renderer independently.
  gulong event_handler = 0;
  gulong add_handler = 0;
  ~ObservedWidget() {
    if (widget == nullptr) return;
    g_signal_handler_disconnect(widget, event_handler);
    if (add_handler != 0) g_signal_handler_disconnect(widget, add_handler);
    g_object_remove_weak_pointer(G_OBJECT(widget), reinterpret_cast<gpointer*>(&widget));
  }
};

NativeTrayMenu::NativeTrayMenu(GtkWindow* window, GtkWidget* view,
                             std::function<void(bool)> keyboard_mode,
                             std::function<void(int64_t)> closed)
    : window_(window), view_(view), keyboard_mode_(std::move(keyboard_mode)),
      closed_(std::move(closed)) {
  g_object_add_weak_pointer(G_OBJECT(window_),
                           reinterpret_cast<gpointer*>(&window_));
  g_object_add_weak_pointer(G_OBJECT(view_),
                           reinterpret_cast<gpointer*>(&view_));
  // The platform message reaches us after Flutter's event dispatch. Retain
  // genuine GTK events beforehand; coordinates and a wall-clock timestamp
  // cannot manufacture the Wayland seat/serial needed by a popup grab.
  // FlView is a GtkBox; its internal GtkEventBox consumes pointer signals.
  // Observe each GTK descendant's generic event before its specific handlers,
  // rather than relying on events bubbling back to FlView/the toplevel.
  ObserveWidget(GTK_WIDGET(window_));
}

NativeTrayMenu::~NativeTrayMenu() {
  destroying_ = true;
  Dismiss();
  observed_.clear();
  cache_.clear();
  if (last_button_ != nullptr) gdk_event_free(last_button_);
  if (last_key_ != nullptr) gdk_event_free(last_key_);
  if (view_ != nullptr) {
    g_object_remove_weak_pointer(G_OBJECT(view_), reinterpret_cast<gpointer*>(&view_));
  }
  if (window_ != nullptr) {
    g_object_remove_weak_pointer(G_OBJECT(window_), reinterpret_cast<gpointer*>(&window_));
  }
}

void NativeTrayMenu::ObserveWidget(GtkWidget* widget) {
  for (const auto& observed : observed_) {
    if (observed->widget == widget) return;
  }
  auto observed = std::make_unique<ObservedWidget>();
  observed->widget = widget;
  g_object_add_weak_pointer(G_OBJECT(widget), reinterpret_cast<gpointer*>(&observed->widget));
  observed->event_handler = g_signal_connect(widget, "event", G_CALLBACK(InputEvent), this);
  if (GTK_IS_CONTAINER(widget)) {
    observed->add_handler = g_signal_connect(widget, "add", G_CALLBACK(ChildAdded), this);
  }
  observed_.push_back(std::move(observed));
  if (GTK_IS_CONTAINER(widget)) gtk_container_forall(GTK_CONTAINER(widget), ObserveChild, this);
}

void NativeTrayMenu::ObserveChild(GtkWidget* child, gpointer data) {
  static_cast<NativeTrayMenu*>(data)->ObserveWidget(child);
}

void NativeTrayMenu::ChildAdded(GtkContainer*, GtkWidget* child, gpointer data) {
  static_cast<NativeTrayMenu*>(data)->ObserveWidget(child);
}

gboolean NativeTrayMenu::InputEvent(GtkWidget*, GdkEvent* event, gpointer data) {
  static_cast<NativeTrayMenu*>(data)->RememberEvent(event);
  return GDK_EVENT_PROPAGATE;
}

void NativeTrayMenu::RememberEvent(GdkEvent* event) {
  if (shown_ && IsKeyEvent(event)) ++parent_key_events_;
  if ((IsPointerEvent(event) || IsKeyEvent(event)) &&
      trace_enabled()) {
    g_message("Native tray input: type=%d synthetic=%d device=%p", event->type,
              event->any.send_event, static_cast<void*>(gdk_event_get_device(event)));
  }
  // Ignore synthetic events and events without a real input device.
  if (event == nullptr || event->any.send_event || gdk_event_get_device(event) == nullptr) return;
  if (active_ != nullptr && !shown_) {
    const bool new_press = (event->type == GDK_BUTTON_PRESS || event->type == GDK_KEY_PRESS) &&
        !SameInput(event, trigger_);
    bool left_parent = false;
    if (event->type == GDK_LEAVE_NOTIFY && event->crossing.mode == GDK_CROSSING_NORMAL &&
        event->crossing.detail != GDK_NOTIFY_INFERIOR) {
      GdkWindow* under_pointer = gdk_device_get_window_at_position(
          gdk_event_get_device(event), nullptr, nullptr);
      left_parent = under_pointer == nullptr || window_ == nullptr ||
          gdk_window_get_toplevel(under_pointer) != gtk_widget_get_window(GTK_WIDGET(window_));
    }
    if (new_press || left_parent || event->type == GDK_SCROLL) {
      Trace(std::string("pending input: type=") + std::to_string(event->type) +
            " newPress=" + std::to_string(new_press) +
            " leftParent=" + std::to_string(left_parent));
      Close(true, new_press ? "new-press" : left_parent ? "left-parent" : "scroll");
    }
  }
  GdkEvent** slot = nullptr;
  if (IsPointerEvent(event)) {
    slot = &last_button_;
    button_time_ = g_get_monotonic_time();
  } else if (IsKeyEvent(event)) {
    slot = &last_key_;
    key_time_ = g_get_monotonic_time();
  }
  if (slot == nullptr) return;
  if (*slot != nullptr) gdk_event_free(*slot);
  *slot = gdk_event_copy(event);
}

NativeTrayMenu::Entry* NativeTrayMenu::GetEntry(const std::string& owner,
                                              const std::string& path) {
  for (const auto& entry : cache_) {
    if (entry->alive && entry->owner == owner && entry->path == path) {
      entry->last_used = ++use_counter_;
      return entry.get();
    }
  }
  if (cache_.size() >= kMaxCachedMenus) {
    auto oldest = std::min_element(cache_.begin(), cache_.end(),
        [](const std::unique_ptr<Entry>& a, const std::unique_ptr<Entry>& b) {
          return a->last_used < b->last_used;
        });
    cache_.erase(oldest);
  }
  auto entry = std::make_unique<Entry>();
  entry->host = this;
  entry->owner = owner;
  entry->path = path;
  entry->last_used = ++use_counter_;
  entry->menu = GTK_WIDGET(dbusmenu_gtkmenu_new(
      const_cast<gchar*>(owner.c_str()), const_cast<gchar*>(path.c_str())));
  g_object_ref_sink(entry->menu);
  gtk_menu_attach_to_widget(GTK_MENU(entry->menu), view_, nullptr);
  gtk_menu_set_reserve_toggle_size(GTK_MENU(entry->menu), FALSE);
  entry->client = DBUSMENU_CLIENT(g_object_ref(
      dbusmenu_gtkmenu_get_client(DBUSMENU_GTKMENU(entry->menu))));
  g_signal_connect(entry->client, DBUSMENU_CLIENT_SIGNAL_ROOT_CHANGED,
                   G_CALLBACK(RootChanged), entry.get());
  g_signal_connect(entry->client, DBUSMENU_CLIENT_SIGNAL_LAYOUT_UPDATED,
                   G_CALLBACK(LayoutUpdated), entry.get());
  g_signal_connect(entry->menu, "deactivate", G_CALLBACK(MenuDeactivated), entry.get());
  g_signal_connect(entry->menu, "unmap", G_CALLBACK(MenuDeactivated), entry.get());
  g_signal_connect(entry->menu, "key-press-event", G_CALLBACK(MenuKeyEvent), entry.get());
  g_signal_connect(entry->menu, "key-release-event", G_CALLBACK(MenuKeyEvent), entry.get());
  entry->owner_watch = g_bus_watch_name(G_BUS_TYPE_SESSION, owner.c_str(),
      G_BUS_NAME_WATCHER_FLAGS_NONE, nullptr, OwnerVanished, entry.get(), nullptr);
  Entry* result = entry.get();
  cache_.push_back(std::move(entry));
  return result;
}

void NativeTrayMenu::Show(const std::string& owner, const std::string& path,
                         int x, int y, int64_t request_id, unsigned button,
                         Complete complete) {
  Dismiss();
  if (trace_enabled()) state_trace_.clear();
  last_wait_.clear();
  Trace("show request=" + std::to_string(request_id) + " button=" + std::to_string(button));
  if (window_ == nullptr || view_ == nullptr || !gtk_widget_get_mapped(GTK_WIDGET(window_)) ||
      !g_dbus_is_unique_name(owner.c_str()) || !g_variant_is_object_path(path.c_str()) ||
      path == "/" || button > 3) {
    Trace("unavailable: invalid-target-or-unmapped-parent");
    complete(Result::Unavailable);
    return;
  }
  g_autoptr(GdkEvent) current = gtk_get_current_event();
  if (current != nullptr) RememberEvent(current);
  const gint64 now = g_get_monotonic_time();
  GdkEvent* event = button == 0 ? last_key_ : last_button_;
  const gint64 age = now - (button == 0 ? key_time_ : button_time_);
  Trace("trigger type=" + std::to_string(event == nullptr ? -1 : event->type) +
        " button=" + std::to_string(event == nullptr || button == 0 ? 0 : event->button.button) +
        " ageUs=" + std::to_string(age));
  if (trace_enabled()) {
    g_message("Native tray show: button=%u event=%d age_us=%" G_GINT64_FORMAT,
              button, event == nullptr ? -1 : event->type, age);
  }
  if (event == nullptr) {
    Trace("unavailable: no-input");
    complete(Result::Unavailable);
    return;
  }
  if (age < 0 || age > kInputMaxAgeUs ||
      (button != 0 && event->button.button != button)) {
    Trace("cancelled: stale-or-replaced-input");
    // A delayed Flutter RPC must not resurrect an older user's action via
    // the SNI fallback after its genuine trigger has expired or changed.
    complete(Result::Cancelled);
    return;
  }
  trigger_ = gdk_event_copy(event);
  active_ = GetEntry(owner, path);
  active_->requested_empty_root = false;
  request_id_ = request_id;
  anchor_ = {x, y, 1, 1};
  complete_ = std::move(complete);
  deadline_ = now + kLayoutTimeoutUs;
  shown_ = false;
  parent_key_events_ = 0;
  menu_key_events_ = 0;
  key_trace_.clear();
  parent_committed_ = false;
  keyboard_mode_(true);
  // Layer keyboard mode is double buffered. Wait for the existing GTK frame
  // to commit the temporary keyboard policy before xdg_popup.grab, without
  // remapping/resizing the Flutter parent or a blocking Wayland roundtrip.
  GdkWindow* parent = gtk_widget_get_window(GTK_WIDGET(window_));
  frame_clock_ = GDK_FRAME_CLOCK(g_object_ref(gdk_window_get_frame_clock(parent)));
  frame_handler_ = g_signal_connect(frame_clock_, "after-paint", G_CALLBACK(AfterPaint), this);
  gdk_window_invalidate_rect(parent, nullptr, FALSE);
  gdk_frame_clock_request_phase(frame_clock_, GDK_FRAME_CLOCK_PHASE_AFTER_PAINT);
  ready_source_ = g_timeout_add(16, CheckReady, this);
}

void NativeTrayMenu::RootChanged(GObject*, gpointer, gpointer data) {
  auto* entry = static_cast<Entry*>(data);
  // GtkMenu's own root handler may run later in this same signal emission;
  // the bounded readiness timer observes its fully materialized widgets.
  entry->requested_empty_root = false;
  entry->host->QueueReflow(entry);
}

void NativeTrayMenu::LayoutUpdated(GObject*, gpointer data) {
  auto* entry = static_cast<Entry*>(data);
  entry->host->QueueReflow(entry);
}

void NativeTrayMenu::QueueReflow(Entry* entry) {
  if (entry->reflow_source == 0) {
    // GtkClient materializes child widgets in the same signal emission.
    // Observe the completed tree once, after all its handlers have run.
    entry->reflow_source = g_idle_add(ReflowEntry, entry);
  }
}

gboolean NativeTrayMenu::ReflowEntry(gpointer data) {
  auto* entry = static_cast<Entry*>(data);
  entry->reflow_source = 0;
  auto* host = entry->host;
  if (!entry->alive) return G_SOURCE_REMOVE;
  host->RefreshSubmenus(entry);
  if (host->active_ != entry || !host->shown_) return G_SOURCE_REMOVE;
  host->Trace("reflow: begin");
  // An initially empty GtkMenu may already have opened while GetLayout
  // was in flight. Coalesce the new layout into its existing native popup;
  // this keeps GTK's grab, keyboard selection and parent relationship.
  gtk_widget_queue_resize(entry->menu);
  gtk_menu_reposition(GTK_MENU(entry->menu));
  for (const auto& submenu : entry->submenus) {
    if (submenu->item == nullptr) continue;
    GtkWidget* menu = gtk_menu_item_get_submenu(submenu->item);
    if (menu != nullptr && gtk_widget_get_mapped(menu)) {
      gtk_widget_queue_resize(menu);
      gtk_menu_reposition(GTK_MENU(menu));
    }
  }
  host->Trace("reflow: end");
  return G_SOURCE_REMOVE;
}

void NativeTrayMenu::RefreshSubmenus(Entry* entry) {
  std::vector<std::shared_ptr<Submenu>> current;
  std::vector<DbusmenuMenuitem*> remaining;
  if (auto* root = dbusmenu_client_get_root(entry->client)) remaining.push_back(root);
  while (!remaining.empty()) {
    auto* model = remaining.back();
    remaining.pop_back();
    for (GList* child = dbusmenu_menuitem_get_children(model); child != nullptr;
         child = child->next) {
      remaining.push_back(DBUSMENU_MENUITEM(child->data));
    }
    auto* item = dbusmenu_gtkclient_menuitem_get(DBUSMENU_GTKCLIENT(entry->client), model);
    if (item == nullptr || gtk_menu_item_get_submenu(item) == nullptr) continue;
    auto found = std::find_if(entry->submenus.begin(), entry->submenus.end(),
        [model, item](const std::shared_ptr<Submenu>& candidate) {
          return candidate->model == model && candidate->item == item;
        });
    if (found != entry->submenus.end()) {
      current.push_back(*found);
      continue;
    }
    auto submenu = std::make_shared<Submenu>();
    submenu->entry = entry;
    submenu->model = model;
    submenu->item = item;
    g_object_add_weak_pointer(G_OBJECT(model), reinterpret_cast<gpointer*>(&submenu->model));
    g_object_add_weak_pointer(G_OBJECT(item), reinterpret_cast<gpointer*>(&submenu->item));
    g_signal_connect(item, "select", G_CALLBACK(SubmenuSelected), submenu.get());
    current.push_back(std::move(submenu));
  }
  for (const auto& previous : entry->submenus) {
    if (std::find(current.begin(), current.end(), previous) == current.end()) previous->Detach();
  }
  entry->submenus = std::move(current);
}

void NativeTrayMenu::SubmenuSelected(GtkMenuItem*, gpointer data) {
  auto* submenu = static_cast<Submenu*>(data);
  if (submenu->entry != nullptr) submenu->entry->host->PrefetchSubmenu(submenu);
}

void NativeTrayMenu::PrefetchSubmenu(Submenu* submenu) {
  Entry* entry = submenu->entry;
  if (active_ != entry || !shown_ || submenu->in_flight || submenu->model == nullptr ||
      submenu->item == nullptr || !gtk_widget_is_sensitive(GTK_WIDGET(submenu->item))) return;
  auto found = std::find_if(entry->submenus.begin(), entry->submenus.end(),
      [submenu](const std::shared_ptr<Submenu>& candidate) { return candidate.get() == submenu; });
  if (found == entry->submenus.end()) return;
  submenu->in_flight = true;
  auto* request = new AboutRequest{entry->lifetime, *found, request_id_, ++submenu->sequence};
  // Unlike GtkClient's activate handler, selection includes ordinary hover.
  // A real callback is essential: libdbusmenu otherwise discards needUpdate
  // in the AboutToShow reply instead of requesting the updated layout.
  dbusmenu_menuitem_send_about_to_show(submenu->model, SubmenuAboutFinished, request);
}

void NativeTrayMenu::SubmenuAboutFinished(DbusmenuMenuitem*, gpointer data) {
  std::unique_ptr<AboutRequest> request(static_cast<AboutRequest*>(data));
  auto* submenu = request->submenu.get();
  if (submenu->sequence != request->sequence) return;
  submenu->in_flight = false;
  Entry* entry = *request->lifetime;
  if (entry != nullptr && submenu->entry == entry && entry->host->active_ == entry &&
      entry->host->request_id_ == request->request_id) entry->host->QueueReflow(entry);
  // About completion precedes the asynchronous GetLayout. LayoutUpdated
  // schedules another idle when actual children have been materialized.
}

void NativeTrayMenu::RootAboutFinished(DbusmenuMenuitem*, gpointer data) {
  std::unique_ptr<AboutRequest> request(static_cast<AboutRequest*>(data));
  Entry* entry = *request->lifetime;
  if (entry != nullptr && entry->host->active_ == entry &&
      entry->host->request_id_ == request->request_id) entry->host->QueueReflow(entry);
}

void NativeTrayMenu::MenuDeactivated(GtkMenuShell* menu, gpointer data) {
  auto* entry = static_cast<Entry*>(data);
  if (entry->host->active_ == entry) {
    const GSignalInvocationHint* signal = g_signal_get_invocation_hint(menu);
    entry->host->Close(true, signal == nullptr ? "menu-close" : g_signal_name(signal->signal_id));
  }
}

gboolean NativeTrayMenu::MenuKeyEvent(GtkWidget*, GdkEvent* event, gpointer data) {
  auto* entry = static_cast<Entry*>(data);
  auto* host = entry->host;
  if (host->active_ == entry) {
    ++host->menu_key_events_;
    if (trace_enabled()) {
      guint keyval = 0;
      guint16 code = 0;
      GdkModifierType state = static_cast<GdkModifierType>(0);
      gdk_event_get_keyval(event, &keyval);
      gdk_event_get_keycode(event, &code);
      gdk_event_get_state(event, &state);
      if (host->key_trace_.size() == 8) host->key_trace_.erase(host->key_trace_.begin());
      host->key_trace_.push_back({keyval, code, static_cast<guint>(state), event->type});
    }
  }
  return GDK_EVENT_PROPAGATE;
}

void NativeTrayMenu::OwnerVanished(GDBusConnection*, const gchar*, gpointer data) {
  auto* entry = static_cast<Entry*>(data);
  entry->alive = false;
  if (entry->host->active_ == entry) entry->host->Close(true, "owner-vanished");
}

void NativeTrayMenu::ClearFrameWait() {
  if (frame_clock_ == nullptr) return;
  if (frame_handler_ != 0) g_signal_handler_disconnect(frame_clock_, frame_handler_);
  frame_handler_ = 0;
  g_clear_object(&frame_clock_);
}

void NativeTrayMenu::AfterPaint(GdkFrameClock*, gpointer data) {
  auto* self = static_cast<NativeTrayMenu*>(data);
  self->parent_committed_ = true;
  self->ClearFrameWait();
}

gboolean NativeTrayMenu::CheckReady(gpointer data) {
  auto* self = static_cast<NativeTrayMenu*>(data);
  if (self->active_ == nullptr) {
    self->ready_source_ = 0;
    return G_SOURCE_REMOVE;
  }
  if (g_get_monotonic_time() >= self->deadline_) {
    self->ready_source_ = 0;
    self->Close(false, "timeout");
    return G_SOURCE_REMOVE;
  }
  self->TryPopup();
  if (self->shown_ || self->active_ == nullptr) {
    self->ready_source_ = 0;
    return G_SOURCE_REMOVE;
  }
  return G_SOURCE_CONTINUE;
}

void NativeTrayMenu::TryPopup() {
  Entry* entry = active_;
  if (entry == nullptr || !entry->alive) return;
  if (!parent_committed_) {
    TraceWait("parent-commit");
    return;
  }
  // The surface commit and its keyboard-enter event are asynchronous. Do
  // not map a visually usable but keyboard-inert menu between those steps.
  if (window_ == nullptr || !gtk_window_is_active(window_)) {
    TraceWait("parent-focus");
    return;
  }
  DbusmenuMenuitem* root = dbusmenu_client_get_root(entry->client);
  if (root == nullptr) {
    TraceWait("root-model");
    return;
  }
  if (!HasVisibleChildren(entry->menu)) {
    TraceWait("root-widgets");
    // Some exporters build even their first root only after AboutToShow.
    if (!entry->requested_empty_root) {
      entry->requested_empty_root = true;
      auto* request = new AboutRequest{entry->lifetime, nullptr, request_id_};
      dbusmenu_menuitem_send_about_to_show(root, RootAboutFinished, request);
    }
    return;
  }
  GtkWidget* menu = entry->menu;
  RefreshSubmenus(entry);
  const int64_t request = request_id_;
  g_autoptr(GdkEvent) event = gdk_event_copy(trigger_);
  g_autoptr(GtkWidget) retained_menu = GTK_WIDGET(g_object_ref(menu));
  Trace("popup: root-ready");
  // Coordinates here are parent-surface logical coordinates, not SNI's
  // global screen coordinates and not physical pixels. GTK/xdg-positioner
  // constrain the popup/submenus to the appropriate output.
  gtk_menu_popup_at_rect(GTK_MENU(menu), gtk_widget_get_window(view_), &anchor_,
      GDK_GRAVITY_SOUTH_WEST, GDK_GRAVITY_NORTH_WEST, event);
  if (active_ != entry || request_id_ != request) return;
  shown_ = gtk_widget_get_mapped(menu);
  Trace(std::string("popup: mapped=") + std::to_string(shown_));
  if (!shown_) {
    Close(false, "map-failed");
    return;
  }
  if (event->type == GDK_KEY_PRESS || event->type == GDK_KEY_RELEASE) {
    gtk_menu_shell_select_first(GTK_MENU_SHELL(menu), TRUE);
  }
  Finish(Result::Shown);
}

void NativeTrayMenu::Finish(Result result) {
  Complete complete = std::move(complete_);
  if (complete) complete(result);
}

void NativeTrayMenu::Dismiss(int64_t request_id) {
  if (active_ != nullptr && request_id_ == request_id) Dismiss();
}

void NativeTrayMenu::Dismiss() {
  Close(true);
}

void NativeTrayMenu::Close(bool cancelled, const char* reason) {
  if (active_ == nullptr) return;
  Trace(std::string("close: ") + reason + " cancelled=" + std::to_string(cancelled) +
        " shown=" + std::to_string(shown_));
  Entry* entry = active_;
  const int64_t request = request_id_;
  const bool was_shown = shown_;
  active_ = nullptr;
  shown_ = false;
  ClearFrameWait();
  if (ready_source_ != 0) {
    g_source_remove(ready_source_);
    ready_source_ = 0;
  }
  gtk_menu_popdown(GTK_MENU(entry->menu));
  if (trigger_ != nullptr) gdk_event_free(trigger_);
  trigger_ = nullptr;
  keyboard_mode_(false);
  Finish(cancelled ? Result::Cancelled : Result::Unavailable);
  // A cold request completes through its pending show result (false for a
  // load failure, menu-cancelled for new input/explicit cancellation). A
  // second close notification would race that result across the channel.
  if (!destroying_ && was_shown) closed_(request);
}

void NativeTrayMenu::Trace(const std::string& message) {
  if (!trace_enabled()) return;
  if (state_trace_.size() == 32) state_trace_.erase(state_trace_.begin());
  state_trace_.push_back(message);
}

void NativeTrayMenu::TraceWait(const char* reason) {
  if (!trace_enabled() || last_wait_ == reason) return;
  last_wait_ = reason;
  Trace(std::string("wait: ") + reason);
}

int NativeTrayMenu::visible_items() const {
  int items = 0, submenus = 0;
  if (active_ != nullptr) CountVisible(active_->menu, &items, &submenus);
  return items;
}

int NativeTrayMenu::visible_submenus() const {
  int items = 0, submenus = 0;
  if (active_ != nullptr) CountVisible(active_->menu, &items, &submenus);
  return submenus;
}

bool NativeTrayMenu::takes_focus() const {
  return active_ != nullptr && gtk_menu_shell_get_take_focus(GTK_MENU_SHELL(active_->menu));
}

bool NativeTrayMenu::has_gtk_grab() const {
  if (active_ == nullptr || window_ == nullptr) return false;
  GtkWidget* grab = gtk_window_group_get_current_grab(gtk_window_get_group(window_));
  while (GTK_IS_MENU_SHELL(grab)) {
    if (grab == active_->menu) return true;
    grab = gtk_menu_shell_get_parent_shell(GTK_MENU_SHELL(grab));
  }
  return false;
}

bool NativeTrayMenu::trace_enabled() {
  return g_strcmp0(g_getenv("CLARION_SHELL_TRAY_TRACE"), "1") == 0;
}

std::vector<NativeTrayMenu::ItemBounds> NativeTrayMenu::item_bounds() const {
  std::vector<ItemBounds> bounds;
  if (active_ != nullptr) CollectBounds(active_->menu, 0, &bounds);
  return bounds;
}
