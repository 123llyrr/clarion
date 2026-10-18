// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef CLARION_SHELL_NATIVE_TRAY_MENU_H_
#define CLARION_SHELL_NATIVE_TRAY_MENU_H_

#include <gtk/gtk.h>

#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <vector>

typedef struct _DbusmenuMenuitem DbusmenuMenuitem;

// GTK owns the widget/DBusMenu protocol; Flutter only supplies an SNI target
// and an anchor in its own view's logical coordinates. All work stays on GTK's
// main context, including the asynchronous first layout and owner watches.
class NativeTrayMenu final {
 public:
  enum class Result { Shown, Unavailable, Cancelled };
  struct KeyTrace {
    guint keyval;
    guint16 hardware_code;
    guint state;
    GdkEventType type;
  };
  struct ItemBounds {
    std::string label;
    int depth;
    int index;
    int x, y, width, height;
    bool enabled;
  };
  using Complete = std::function<void(Result)>;
  NativeTrayMenu(GtkWindow* window, GtkWidget* view,
                 std::function<void(bool)> keyboard_mode,
                 std::function<void(int64_t)> closed);
  ~NativeTrayMenu();
  NativeTrayMenu(const NativeTrayMenu&) = delete;
  NativeTrayMenu& operator=(const NativeTrayMenu&) = delete;

  void Show(const std::string& owner, const std::string& path, int x, int y,
            int64_t request_id, unsigned button, Complete complete);
  void Dismiss();
  void Dismiss(int64_t request_id);
  bool active() const { return active_ != nullptr; }
  int64_t request_id() const { return active() ? request_id_ : -1; }
  int visible_items() const;
  int visible_submenus() const;
  bool takes_focus() const;
  bool has_gtk_grab() const;
  unsigned parent_key_events() const { return parent_key_events_; }
  unsigned menu_key_events() const { return menu_key_events_; }
  static bool trace_enabled();
  const std::vector<KeyTrace>& key_trace() const { return key_trace_; }
  const std::vector<std::string>& state_trace() const { return state_trace_; }
  std::vector<ItemBounds> item_bounds() const;

 private:
  struct Entry;
  struct ObservedWidget;
  struct Submenu;
  struct AboutRequest;
  static gboolean InputEvent(GtkWidget*, GdkEvent*, gpointer);
  static void ObserveChild(GtkWidget*, gpointer);
  static void ChildAdded(GtkContainer*, GtkWidget*, gpointer);
  static void RootChanged(GObject*, gpointer, gpointer);
  static void LayoutUpdated(GObject*, gpointer);
  static void SubmenuSelected(GtkMenuItem*, gpointer);
  static void SubmenuAboutFinished(DbusmenuMenuitem*, gpointer);
  static void RootAboutFinished(DbusmenuMenuitem*, gpointer);
  static gboolean ReflowEntry(gpointer);
  static void MenuDeactivated(GtkMenuShell*, gpointer);
  static gboolean MenuKeyEvent(GtkWidget*, GdkEvent*, gpointer);
  static void OwnerVanished(GDBusConnection*, const gchar*, gpointer);
  static gboolean CheckReady(gpointer);
  static void AfterPaint(GdkFrameClock*, gpointer);
  void RememberEvent(GdkEvent* event);
  void ObserveWidget(GtkWidget* widget);
  void QueueReflow(Entry* entry);
  void RefreshSubmenus(Entry* entry);
  void PrefetchSubmenu(Submenu* submenu);
  Entry* GetEntry(const std::string& owner, const std::string& path);
  void TryPopup();
  void Finish(Result result);
  void Close(bool cancelled, const char* reason = "dismiss");
  void Trace(const std::string& message);
  void TraceWait(const char* reason);
  void ClearFrameWait();

  GtkWindow* window_;
  GtkWidget* view_;
  std::function<void(bool)> keyboard_mode_;
  std::function<void(int64_t)> closed_;
  std::vector<std::unique_ptr<Entry>> cache_;
  std::vector<std::unique_ptr<ObservedWidget>> observed_;
  Entry* active_ = nullptr;
  GdkEvent* last_button_ = nullptr;
  GdkEvent* last_key_ = nullptr;
  GdkEvent* trigger_ = nullptr;
  gint64 button_time_ = 0;
  gint64 key_time_ = 0;
  gint64 deadline_ = 0;
  uint64_t use_counter_ = 0;
  int64_t request_id_ = -1;
  GdkRectangle anchor_ = {0, 0, 1, 1};
  Complete complete_;
  guint ready_source_ = 0;
  GdkFrameClock* frame_clock_ = nullptr;
  gulong frame_handler_ = 0;
  bool parent_committed_ = false;
  bool shown_ = false;
  bool destroying_ = false;
  unsigned parent_key_events_ = 0;
  unsigned menu_key_events_ = 0;
  std::vector<KeyTrace> key_trace_;
  std::vector<std::string> state_trace_;
  std::string last_wait_;
};

#endif  // CLARION_SHELL_NATIVE_TRAY_MENU_H_
