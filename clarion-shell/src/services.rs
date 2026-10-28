//! Native desktop services. UI commands never perform D-Bus or clipboard I/O.
mod bus;
mod clipboard;
mod history;
#[cfg(test)]
mod native_tests;

use std::time::{SystemTime, UNIX_EPOCH};

use async_channel::{Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub id: u32,
    pub app: String,
    /// The icon the application offered in `Notify`: an absolute path or a
    /// freedesktop icon name. `None` means the card falls back to the app's
    /// catalog artwork, then to a generic bell.
    pub icon: Option<String>,
    pub title: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub created_at: u64,
    pub unread: bool,
    /// Pinned notifications stay in the center and are never expired by the
    /// service; the card shows the pin state in its header.
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayItem {
    pub id: String,
    pub owner: String,
    pub path: String,
    pub title: String,
    pub icon_name: String,
    pub icon_png: Option<Vec<u8>>,
    pub status: String,
    pub menu_path: Option<String>,
    pub item_is_menu: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryKind {
    Text,
    Image,
    Screenshot,
    /// A `text/uri-list` capture whose entries are all local files.
    Files,
    /// An opaque payload the shell cannot render, e.g. a PDF or a video the
    /// application offered as raw bytes. Videos are not their own class: they
    /// ride the file tag with the other non-text payloads.
    Binary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: String,
    pub kind: HistoryKind,
    pub text: String,
    pub thumbnail_png: Option<Vec<u8>>,
    pub pinned: bool,
    pub created_at: u64,
    pub file_path: Option<std::path::PathBuf>,
    /// The clipboard offer this entry came from, e.g. `text/plain`.
    pub mime: String,
    /// The payload size in bytes, before any thumbnail re-encoding.
    pub bytes: usize,
    /// Pixel size for image and screenshot entries, read while thumbnailing.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub separator: bool,
    pub toggle: Option<bool>,
    /// The owner's device/menu icon: a freedesktop icon name when given.
    pub icon_name: Option<String>,
    /// The same icon as raw bytes (dbusmenu `icon-data`), already normalized
    /// to PNG so the view can cache and paint it directly.
    pub icon_png: Option<Vec<u8>>,
    pub children: Vec<MenuItem>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Notifications(Vec<Notification>),
    Tray(Vec<TrayItem>),
    History {
        entries: Vec<HistoryEntry>,
        paused: bool,
        available: bool,
    },
    TrayMenu {
        id: String,
        items: Vec<MenuItem>,
    },
    Error(String),
}

#[derive(Clone, Debug)]
pub enum Command {
    Copy(String),
    Pin(String),
    Delete(String),
    Clear,
    Pause(bool),
    RecordScreenshot(std::path::PathBuf),
    TrayActivate { id: String, x: i32, y: i32 },
    GetMenu(String),
    MenuClick { id: String, item: i32 },
    NotificationAction { id: u32, action: String },
    DismissNotification(u32),
    /// Pin or unpin one notification. Pinned entries stop expiring and sort
    /// before the live feed.
    PinNotification { id: u32, pinned: bool },
    MarkRead,
    Dnd(bool),
    Shutdown,
}

pub struct ServiceHandle {
    commands: Sender<Command>,
}

impl ServiceHandle {
    pub fn send(&self, command: Command) {
        let _ = self.commands.try_send(command);
    }

    /// Test-only: keep every command on a channel instead of starting the
    /// native services, so a test can assert what the UI asked for.
    #[cfg(test)]
    pub fn test_channel(capacity: usize) -> (Self, Receiver<Command>) {
        let (commands, receiver) = async_channel::bounded(capacity);
        (Self { commands }, receiver)
    }
}
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Command::Shutdown);
    }
}

pub fn start(sender: Sender<Event>, preview: bool) -> ServiceHandle {
    let (commands, receiver) = async_channel::unbounded();
    if preview {
        let _ = sender.try_send(Event::Tray(Vec::new()));
        let _ = sender.try_send(Event::Notifications(Vec::new()));
        let _ = sender.try_send(Event::History {
            entries: Vec::new(),
            paused: false,
            available: false,
        });
        return ServiceHandle { commands };
    }
    let (clipboard_commands, clipboard_rx) = async_channel::unbounded();
    let (bus_commands, bus_rx) = async_channel::unbounded();
    let clipboard_events = sender.clone();
    let _ = std::thread::Builder::new()
        .name("clarion-clipboard".into())
        .spawn(move || {
            if let Err(error) = clipboard::run(clipboard_events.clone(), clipboard_rx) {
                let _ =
                    clipboard_events.send_blocking(Event::Error(format!("剪贴板服务：{error:#}")));
            }
        });
    let bus_events = sender;
    let _ = std::thread::Builder::new()
        .name("clarion-desktop-bus".into())
        .spawn(move || {
            if let Err(error) = async_io::block_on(bus::run(bus_events.clone(), bus_rx)) {
                let _ = bus_events.send_blocking(Event::Error(format!("桌面服务：{error:#}")));
            }
        });
    let _ = std::thread::Builder::new()
        .name("clarion-service-router".into())
        .spawn(move || {
            route(receiver, clipboard_commands, bus_commands);
        });
    ServiceHandle { commands }
}

fn route(receiver: Receiver<Command>, clipboard: Sender<Command>, bus: Sender<Command>) {
    while let Ok(command) = receiver.recv_blocking() {
        if matches!(command, Command::Shutdown) {
            let _ = clipboard.send_blocking(Command::Shutdown);
            let _ = bus.send_blocking(Command::Shutdown);
            return;
        }
        let destination = if matches!(
            command,
            Command::Copy(_)
                | Command::Pin(_)
                | Command::Delete(_)
                | Command::Clear
                | Command::Pause(_)
                | Command::RecordScreenshot(_)
        ) {
            &clipboard
        } else {
            &bus
        };
        let _ = destination.send_blocking(command);
    }
    let _ = clipboard.send_blocking(Command::Shutdown);
    let _ = bus.send_blocking(Command::Shutdown);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
