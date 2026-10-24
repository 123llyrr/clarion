use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use futures_lite::{future, StreamExt};
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection, Proxy};

use super::{now_ms, Command, Event, MenuItem, Notification, TrayItem};

const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
const NOTIFY_IFACE: &str = "org.freedesktop.Notifications";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const KDE_WATCHER: &str = "org.kde.StatusNotifierWatcher";
const FDO_WATCHER: &str = "org.freedesktop.StatusNotifierWatcher";
const ITEM_IFACES: [&str; 2] = [
    "org.kde.StatusNotifierItem",
    "org.freedesktop.StatusNotifierItem",
];
const MENU_IFACE: &str = "com.canonical.dbusmenu";
type Properties = HashMap<String, OwnedValue>;
type Layout = (i32, Properties, Vec<OwnedValue>);

struct StoredNotification {
    item: Notification,
    owner: String,
    expires: Option<Instant>,
    active: bool,
}
struct NotificationsState {
    items: HashMap<u32, StoredNotification>,
    next_id: u32,
    dnd: bool,
}
#[derive(Clone)]
struct Notifications {
    state: Arc<Mutex<NotificationsState>>,
    events: Sender<Event>,
}

impl Notifications {
    fn snapshot(&self) {
        let state = self.state.lock().unwrap();
        let mut items: Vec<_> = state
            .items
            .values()
            .map(|stored| stored.item.clone())
            .collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
        let _ = self.events.try_send(Event::Notifications(items));
    }
    async fn close(&self, connection: &Connection, id: u32, reason: u32, remove: bool) {
        let owner = {
            let mut state = self.state.lock().unwrap();
            let owner = state
                .items
                .get_mut(&id)
                .filter(|stored| stored.active)
                .map(|stored| {
                    stored.active = false;
                    stored.owner.clone()
                });
            if remove {
                state.items.remove(&id);
            }
            owner
        };
        if let Some(owner) = owner {
            let _ = connection
                .emit_signal(
                    Some(owner.as_str()),
                    NOTIFY_PATH,
                    NOTIFY_IFACE,
                    "NotificationClosed",
                    &(id, reason),
                )
                .await;
        }
        self.snapshot();
    }
}

#[interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    fn get_capabilities(&self) -> Vec<&str> {
        vec!["actions", "body", "persistence"]
    }
    fn get_server_information(&self) -> (&str, &str, &str, &str) {
        ("Clarion", "Clarion", env!("CARGO_PKG_VERSION"), "1.2")
    }
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        _hints: Properties,
        expire_timeout: i32,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<u32> {
        if app_name.len() > 4096
            || summary.len() > 16384
            || body.len() > 65536
            || actions.len() > 64
        {
            return Err(zbus::fdo::Error::InvalidArgs("通知内容超过限制".into()));
        }
        let owner = header
            .sender()
            .context("notification sender missing")
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?
            .to_string();
        let mut state = self.state.lock().unwrap();
        let id = if replaces_id != 0
            && state
                .items
                .get(&replaces_id)
                .is_some_and(|item| item.owner == owner)
        {
            replaces_id
        } else {
            loop {
                state.next_id = state.next_id.wrapping_add(1).max(1);
                if !state.items.contains_key(&state.next_id) {
                    break state.next_id;
                }
            }
        };
        let expires = if expire_timeout == 0 {
            None
        } else {
            Some(
                Instant::now()
                    + Duration::from_millis(if expire_timeout < 0 {
                        5000
                    } else {
                        expire_timeout.min(3600000) as u64
                    }),
            )
        };
        let dnd = state.dnd;
        state.items.insert(
            id,
            StoredNotification {
                item: Notification {
                    id,
                    app: app_name.into(),
                    title: summary.into(),
                    body: body.into(),
                    actions: actions
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|a| (a[0].clone(), a[1].clone()))
                        .collect(),
                    created_at: now_ms(),
                    unread: !dnd,
                },
                owner,
                expires,
                active: true,
            },
        );
        while state.items.len() > 100 {
            let Some(oldest) = state
                .items
                .values()
                .min_by_key(|item| item.item.created_at)
                .map(|item| item.item.id)
            else {
                break;
            };
            state.items.remove(&oldest);
        }
        drop(state);
        self.snapshot();
        Ok(id)
    }
    async fn close_notification(
        &self,
        id: u32,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let permitted = self
            .state
            .lock()
            .unwrap()
            .items
            .get(&id)
            .is_some_and(|stored| {
                header
                    .sender()
                    .is_some_and(|owner| owner.as_str() == stored.owner)
            });
        if permitted {
            self.close(connection, id, 3, false).await;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Registration {
    service: String,
    path: String,
    sender: String,
}
#[derive(Clone)]
struct WatcherState {
    registered: Arc<Mutex<Vec<String>>>,
    pending: Sender<Registration>,
}
impl WatcherState {
    fn register(&self, service: &str, header: Header<'_>) -> zbus::fdo::Result<()> {
        if service.is_empty() || service.len() > 512 {
            return Err(zbus::fdo::Error::InvalidArgs("托盘标识无效".into()));
        }
        let sender = header
            .sender()
            .context("missing sender")
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?
            .to_string();
        let (service, path): (String, String) = if service.starts_with('/') {
            (sender.clone(), service.into())
        } else {
            (service.into(), "/StatusNotifierItem".into())
        };
        if OwnedObjectPath::try_from(path.as_str()).is_err() {
            return Err(zbus::fdo::Error::InvalidArgs("托盘路径无效".into()));
        }
        let _ = self.pending.try_send(Registration {
            service,
            path,
            sender,
        });
        Ok(())
    }
}
struct KdeWatcher(WatcherState);
#[interface(name = "org.kde.StatusNotifierWatcher")]
impl KdeWatcher {
    fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        self.0.register(service, header)
    }
    fn register_status_notifier_host(&self, _service: &str) {}
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.0.registered.lock().unwrap().clone()
    }
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}
struct FdoWatcher(WatcherState);
#[interface(name = "org.freedesktop.StatusNotifierWatcher")]
impl FdoWatcher {
    fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        self.0.register(service, header)
    }
    fn register_status_notifier_host(&self, _service: &str) {}
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.0.registered.lock().unwrap().clone()
    }
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

struct Registered {
    service: String,
    owner: String,
    path: String,
    interface: String,
    item: TrayItem,
    /// The layout as it was last fetched: the popup displays this exact list,
    /// so a click is validated against it instead of a fresh re-fetch that
    /// could disagree with what the user is looking at.
    menu: Vec<MenuItem>,
    _signals: Vec<zbus::Task<()>>,
}

fn watch_item(
    connection: &Connection,
    owner: &str,
    path: &str,
    refresh: &Sender<()>,
) -> Vec<zbus::Task<()>> {
    [
        ITEM_IFACES[0],
        ITEM_IFACES[1],
        "org.freedesktop.DBus.Properties",
    ]
    .into_iter()
    .map(|interface| {
        let connection_clone = connection.clone();
        let owner = owner.to_owned();
        let path = path.to_owned();
        let refresh = refresh.clone();
        connection.executor().spawn(
            async move {
                if let Ok(proxy) = Proxy::new_owned(connection_clone, owner, path, interface).await
                {
                    if let Ok(mut signals) = proxy.receive_all_signals().await {
                        while signals.next().await.is_some() {
                            let _ = refresh.try_send(());
                        }
                    }
                }
            },
            "Clarion tray properties",
        )
    })
    .collect()
}

/// The menu owner publishes layout revisions itself: `LayoutUpdated` after it
/// rewrote the tree and `ItemsPropertiesUpdated` when toggles flip state.
/// Watching them keeps the displayed menu (rows, ids and check states) in step
/// with the owner without polling.
fn watch_menu(
    connection: &Connection,
    id: &str,
    owner: &str,
    menu_path: &str,
    reload: &Sender<String>,
) -> Vec<zbus::Task<()>> {
    ["LayoutUpdated", "ItemsPropertiesUpdated"]
        .into_iter()
        .map(|signal| {
            let connection_clone = connection.clone();
            let owner = owner.to_owned();
            let path = menu_path.to_owned();
            let reload = reload.clone();
            let id = id.to_owned();
            connection.executor().spawn(
                async move {
                    if let Ok(proxy) =
                        Proxy::new_owned(connection_clone, owner, path, MENU_IFACE).await
                    {
                        if let Ok(mut signals) = proxy.receive_signal(signal).await {
                            while signals.next().await.is_some() {
                                let _ = reload.try_send(id.clone());
                            }
                        }
                    }
                },
                "Clarion tray menu",
            )
        })
        .collect()
}

async fn owner(connection: &Connection, name: &str) -> Result<String> {
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    Ok(dbus.get_name_owner(name.try_into()?).await?.to_string())
}
async fn properties(
    connection: &Connection,
    owner: &str,
    path: &str,
    interface: &str,
) -> Result<Properties> {
    let proxy = Proxy::new(connection, owner, path, "org.freedesktop.DBus.Properties").await?;
    Ok(proxy.call("GetAll", &(interface,)).await?)
}
fn string(properties: &Properties, name: &str) -> String {
    properties
        .get(name)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or_default()
        .to_owned()
}
fn boolean(properties: &Properties, name: &str, default: bool) -> bool {
    properties
        .get(name)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(default)
}

fn argb_png(width: i32, height: i32, argb: Vec<u8>) -> Option<Vec<u8>> {
    if width <= 0 || height <= 0 || width > 1024 || height > 1024 {
        return None;
    }
    if argb.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let mut rgba = Vec::with_capacity(argb.len());
    for pixel in argb.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[pixel[1], pixel[2], pixel[3], pixel[0]]);
    }
    let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba)?;
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .thumbnail(64, 64)
        .write_to(&mut output, image::ImageFormat::Png)
        .ok()?;
    Some(output.into_inner())
}

fn icon(properties: &Properties, name: &str) -> Option<Vec<u8>> {
    let values: Vec<(i32, i32, Vec<u8>)> =
        properties.get(name)?.try_clone().ok()?.try_into().ok()?;
    let mut images: Vec<_> = values
        .into_iter()
        .filter(|(w, h, bytes)| {
            *w > 0
                && *h > 0
                && *w <= 1024
                && *h <= 1024
                && bytes.len() == (*w as usize) * (*h as usize) * 4
        })
        .collect();
    images.sort_by_key(|(w, _, _)| (*w - 32).abs());
    let (width, height, argb) = images.into_iter().next()?;
    argb_png(width, height, argb)
}

/// dbusmenu packs `icon-data` as one `(iiay)` struct; some owners send the
/// SNI-style array, others a raw encoded image. All normalize to a PNG the
/// view can cache.
fn menu_icon(properties: &Properties, name: &str) -> Option<Vec<u8>> {
    let value = properties.get(name)?.try_clone().ok()?;
    if let Ok((width, height, argb)) = <(i32, i32, Vec<u8>)>::try_from(value.clone()) {
        return argb_png(width, height, argb);
    }
    if let Ok(images) = <Vec<(i32, i32, Vec<u8>)>>::try_from(value.clone()) {
        let mut best: Option<(usize, Vec<u8>)> = None;
        for (width, height, argb) in images {
            if let Some(png) = argb_png(width, height, argb) {
                let near = (width - 32).unsigned_abs() as usize;
                if best.as_ref().is_none_or(|(size, _)| near < *size) {
                    best = Some((near, png));
                }
            }
        }
        if best.is_some() {
            return best.map(|(_, png)| png);
        }
    }
    // A raw `ay` byte string: an already-encoded image, usually PNG.
    let raw = <Vec<u8>>::try_from(value).ok()?;
    let decoded = image::load_from_memory(&raw).ok()?;
    let mut output = Cursor::new(Vec::new());
    decoded
        .thumbnail(64, 64)
        .write_to(&mut output, image::ImageFormat::Png)
        .ok()?;
    Some(output.into_inner())
}

async fn read_item(connection: &Connection, registration: &mut Registered) -> Result<()> {
    let mut found = None;
    for interface in [
        registration.interface.as_str(),
        ITEM_IFACES[0],
        ITEM_IFACES[1],
    ] {
        if let Ok(properties) = properties(
            connection,
            &registration.owner,
            &registration.path,
            interface,
        )
        .await
        {
            found = Some((interface.to_owned(), properties));
            break;
        }
    }
    let (interface, p) = found.context("无法读取托盘项目")?;
    registration.interface = interface;
    let attention = string(&p, "Status") == "NeedsAttention";
    let icon_name = if attention {
        string(&p, "AttentionIconName")
    } else {
        String::new()
    };
    let menu = p
        .get("Menu")
        .and_then(|v| <&zbus::zvariant::ObjectPath<'_>>::try_from(v).ok())
        .map(ToString::to_string)
        .filter(|path| path != "/NO_DBUSMENU" && path != "/");
    let title = string(&p, "Title");
    registration.item = TrayItem {
        id: format!("{}{}", registration.owner, registration.path),
        owner: registration.owner.clone(),
        path: registration.path.clone(),
        title: if title.is_empty() {
            string(&p, "Id")
        } else {
            title
        },
        icon_name: if icon_name.is_empty() {
            string(&p, "IconName")
        } else {
            icon_name
        },
        icon_png: if attention {
            icon(&p, "AttentionIconPixmap").or_else(|| icon(&p, "IconPixmap"))
        } else {
            icon(&p, "IconPixmap")
        },
        status: string(&p, "Status"),
        menu_path: menu,
        item_is_menu: boolean(&p, "ItemIsMenu", false),
    };
    Ok(())
}

fn parse_layout(layout: Layout, depth: usize, budget: &mut usize) -> Option<MenuItem> {
    if depth > 8 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let (id, p, children) = layout;
    let children = children
        .into_iter()
        .take(256)
        .filter_map(|value| Layout::try_from(value).ok())
        .filter_map(|child| parse_layout(child, depth + 1, budget))
        .collect();
    let toggle = if string(&p, "toggle-type").is_empty() {
        None
    } else {
        Some(
            p.get("toggle-state")
                .and_then(|value| i32::try_from(value).ok())
                .unwrap_or(0)
                > 0,
        )
    };
    Some(MenuItem {
        id,
        label: string(&p, "label")
            .replace("__", "\u{0}")
            .replace('_', "")
            .replace('\u{0}', "_"),
        enabled: boolean(&p, "enabled", true),
        visible: boolean(&p, "visible", true),
        separator: string(&p, "type") == "separator",
        toggle,
        icon_name: {
            let name = string(&p, "icon-name");
            if name.is_empty() { None } else { Some(name) }
        },
        icon_png: menu_icon(&p, "icon-data"),
        children,
    })
}
async fn get_menu(connection: &Connection, item: &Registered) -> Result<Vec<MenuItem>> {
    let path = item.item.menu_path.as_ref().context("此托盘项目没有菜单")?;
    let proxy = Proxy::new(connection, item.owner.as_str(), path.as_str(), MENU_IFACE).await?;
    let _: std::result::Result<bool, _> = proxy.call("AboutToShow", &(0i32,)).await;
    let (_, layout): (u32, Layout) = proxy
        .call("GetLayout", &(0i32, -1i32, Vec::<String>::new()))
        .await?;
    Ok(parse_layout(layout, 0, &mut 512)
        .map(|item| item.children)
        .unwrap_or_default())
}

async fn register(
    connection: &Connection,
    registration: Registration,
    items: &mut HashMap<String, Registered>,
    refresh: &Sender<()>,
    reload: &Sender<String>,
) -> Result<bool> {
    if items.len() >= 128 {
        return Ok(false);
    }
    let unique = owner(connection, &registration.service).await?;
    // An application can register a bus it owns; it cannot impersonate a
    // different process's unique owner by supplying an unrelated service name.
    anyhow::ensure!(
        registration.sender.is_empty() || registration.sender == unique,
        "托盘注册的所有者不匹配"
    );
    let id = format!("{}{}", unique, registration.path);
    if items.contains_key(&id) {
        return Ok(false);
    }
    let signals = watch_item(connection, &unique, &registration.path, refresh);
    let mut item = Registered {
        service: registration.service,
        owner: unique.clone(),
        path: registration.path.clone(),
        interface: ITEM_IFACES[0].into(),
        item: TrayItem {
            id: id.clone(),
            owner: unique,
            path: registration.path,
            title: String::new(),
            icon_name: String::new(),
            icon_png: None,
            status: String::new(),
            menu_path: None,
            item_is_menu: false,
        },
        _signals: signals,
        menu: Vec::new(),
    };
    read_item(connection, &mut item).await?;
    if let Some(menu_path) = item.item.menu_path.clone() {
        item._signals
            .extend(watch_menu(connection, &id, &item.owner, &menu_path, reload));
    }
    items.insert(id, item);
    Ok(true)
}

fn emit_tray(events: &Sender<Event>, items: &HashMap<String, Registered>) {
    let mut snapshot: Vec<_> = items.values().map(|item| item.item.clone()).collect();
    snapshot.sort_by(|a, b| a.id.cmp(&b.id));
    let _ = events.try_send(Event::Tray(snapshot));
}
async fn watcher_signal(connection: &Connection, name: &str, item: &str) {
    for interface in [KDE_WATCHER, FDO_WATCHER] {
        let _ = connection
            .emit_signal(None::<&str>, WATCHER_PATH, interface, name, &(item,))
            .await;
    }
}

pub(super) async fn run(events: Sender<Event>, commands: Receiver<Command>) -> Result<()> {
    let connection = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_millis(900))
        .build()
        .await?;
    let notifications = Notifications {
        state: Arc::new(Mutex::new(NotificationsState {
            items: HashMap::new(),
            next_id: 0,
            dnd: false,
        })),
        events: events.clone(),
    };
    connection
        .object_server()
        .at(NOTIFY_PATH, notifications.clone())
        .await?;
    let notification_owned = connection.request_name(NOTIFY_IFACE).await.is_ok();
    if !notification_owned {
        let _ = events.try_send(Event::Error("已有通知服务占用名称，Clarion未替换它".into()));
    }
    let (registrations, pending) = async_channel::unbounded();
    let watcher = WatcherState {
        registered: Arc::new(Mutex::new(Vec::new())),
        pending: registrations,
    };
    connection
        .object_server()
        .at(WATCHER_PATH, KdeWatcher(watcher.clone()))
        .await?;
    connection
        .object_server()
        .at(WATCHER_PATH, FdoWatcher(watcher.clone()))
        .await?;
    let kde_owned = connection.request_name(KDE_WATCHER).await.is_ok();
    let fdo_owned = connection.request_name(FDO_WATCHER).await.is_ok();
    let host_name = format!("org.kde.StatusNotifierHost.Clarion{}", std::process::id());
    let _ = connection.request_name(host_name.as_str()).await;
    let mut items: HashMap<String, Registered> = HashMap::new();
    let (refresh, refreshed) = async_channel::bounded(1);
    let (menu_reload, menu_reloaded) = async_channel::bounded::<String>(64);
    let owner_connection = connection.clone();
    let owner_refresh = refresh.clone();
    let _owners = connection.executor().spawn(
        async move {
            if let Ok(proxy) = zbus::fdo::DBusProxy::new(&owner_connection).await {
                if let Ok(mut stream) = proxy.receive_name_owner_changed().await {
                    while stream.next().await.is_some() {
                        let _ = owner_refresh.try_send(());
                    }
                }
            }
        },
        "Clarion tray owners",
    );
    let mut tick = Instant::now() - Duration::from_secs(3);
    notifications.snapshot();
    emit_tray(&events, &items);
    loop {
        while let Ok(registration) = pending.try_recv() {
            let key = format!("{}{}", registration.service, registration.path);
            if register(&connection, registration, &mut items, &refresh, &menu_reload)
                .await
                .unwrap_or(false)
            {
                watcher_signal(&connection, "StatusNotifierItemRegistered", &key).await;
                emit_tray(&events, &items);
            }
        }
        if refreshed.try_recv().is_ok() {
            tick = Instant::now() - Duration::from_secs(3);
        }
        // Menu owners republished their layout: catch each mentioned item up
        // with its current tree so an open popup repaints the new row state.
        let mut reloaded: Vec<String> = Vec::new();
        while let Ok(id) = menu_reloaded.try_recv() {
            if !reloaded.contains(&id) {
                reloaded.push(id);
            }
        }
        for id in reloaded {
            if let Some(item) = items.get_mut(&id) {
                if let Ok(menu) = get_menu(&connection, item).await {
                    item.menu = menu.clone();
                    let _ = events.try_send(Event::TrayMenu { id, items: menu });
                }
            }
        }
        if tick.elapsed() >= Duration::from_secs(2) {
            tick = Instant::now();
            // Existing watchers remain authoritative. Register as a real host
            // and import their current items rather than replacing their name.
            for (name, owned) in [(KDE_WATCHER, kde_owned), (FDO_WATCHER, fdo_owned)] {
                if !owned {
                    if let Ok(proxy) = Proxy::new(&connection, name, WATCHER_PATH, name).await {
                        let _: std::result::Result<(), _> = proxy
                            .call("RegisterStatusNotifierHost", &(host_name.as_str(),))
                            .await;
                        if let Ok(registered) = proxy
                            .get_property::<Vec<String>>("RegisteredStatusNotifierItems")
                            .await
                        {
                            for value in registered.into_iter().take(128) {
                                let Some(index) = value.find('/') else {
                                    continue;
                                };
                                let registration = Registration {
                                    service: value[..index].into(),
                                    path: value[index..].into(),
                                    sender: String::new(),
                                };
                                let _ = register(
                                    &connection,
                                    registration,
                                    &mut items,
                                    &refresh,
                                    &menu_reload,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
            let ids: Vec<_> = items.keys().cloned().collect();
            let mut changed = false;
            for id in ids {
                let item = items.get_mut(&id).unwrap();
                if owner(&connection, &item.service).await.ok().as_deref()
                    != Some(item.owner.as_str())
                {
                    let registration = format!("{}{}", item.service, item.path);
                    items.remove(&id);
                    watcher_signal(&connection, "StatusNotifierItemUnregistered", &registration)
                        .await;
                    changed = true;
                    continue;
                }
                let before = item.item.clone();
                let _ = read_item(&connection, item).await;
                changed |= before != item.item;
            }
            let registered = items
                .values()
                .map(|item| format!("{}{}", item.owner, item.path))
                .collect();
            *watcher.registered.lock().unwrap() = registered;
            if changed {
                emit_tray(&events, &items);
            }
            let expired: Vec<_> = notifications
                .state
                .lock()
                .unwrap()
                .items
                .values()
                .filter(|item| item.active && item.expires.is_some_and(|due| Instant::now() >= due))
                .map(|item| item.item.id)
                .collect();
            for id in expired {
                notifications.close(&connection, id, 1, false).await;
            }
        }
        let command = future::or(async { commands.recv().await.ok() }, async {
            async_io::Timer::after(Duration::from_millis(50)).await;
            None
        })
        .await;
        let Some(command) = command else {
            if commands.is_closed() {
                return Ok(());
            }
            continue;
        };
        let shutdown = matches!(command, Command::Shutdown);
        let result: Result<()> = async {
            match command {
                Command::Shutdown => return Ok(()),
                Command::Dnd(value) => {
                    notifications.state.lock().unwrap().dnd = value;
                }
                Command::MarkRead => {
                    for item in notifications.state.lock().unwrap().items.values_mut() {
                        item.item.unread = false;
                    }
                    notifications.snapshot();
                }
                Command::DismissNotification(id) => {
                    notifications.close(&connection, id, 2, true).await
                }
                Command::NotificationAction { id, action } => {
                    let owner = notifications
                        .state
                        .lock()
                        .unwrap()
                        .items
                        .get(&id)
                        .filter(|item| {
                            item.active && item.item.actions.iter().any(|(key, _)| key == &action)
                        })
                        .map(|item| item.owner.clone());
                    if let Some(owner) = owner {
                        connection
                            .emit_signal(
                                Some(owner.as_str()),
                                NOTIFY_PATH,
                                NOTIFY_IFACE,
                                "ActionInvoked",
                                &(id, action),
                            )
                            .await?;
                        notifications.close(&connection, id, 2, false).await;
                    }
                }
                Command::TrayActivate { id, x, y } => {
                    let item = items.get_mut(&id).context("托盘项目已退出")?;
                    if item.item.item_is_menu {
                        let menu = get_menu(&connection, item).await?;
                        item.menu = menu.clone();
                        let _ = events.try_send(Event::TrayMenu { id, items: menu });
                    } else {
                        let proxy = Proxy::new(
                            &connection,
                            item.owner.as_str(),
                            item.path.as_str(),
                            item.interface.as_str(),
                        )
                        .await?;
                        let _: () = proxy.call("Activate", &(x, y)).await?;
                    }
                }
                Command::GetMenu(id) => {
                    let item = items.get_mut(&id).context("托盘项目已退出")?;
                    let menu = get_menu(&connection, item).await?;
                    item.menu = menu.clone();
                    let _ = events.try_send(Event::TrayMenu { id, items: menu });
                }
                Command::MenuClick { id, item: menu_id } => {
                    let item = items.get_mut(&id).context("托盘项目已退出")?;
                    // The popup displays the layout fetched when it opened:
                    // validate against that stored list rather than re-fetching
                    // a fresh one, which rejects rows the application restated
                    // between the menu being drawn and the click. Disabled
                    // rows never paint a button, so the displayed list is the
                    // authority for what is clickable.
                    fn enabled(items: &[MenuItem], id: i32) -> bool {
                        items.iter().any(|item| {
                            (item.id == id && item.enabled && item.visible && !item.separator)
                                || enabled(&item.children, id)
                        })
                    }
                    anyhow::ensure!(enabled(&item.menu, menu_id), "菜单项目不可用");
                    let owner = item.owner.clone();
                    let path = item.item.menu_path.clone().context("托盘菜单不可用")?;
                    let proxy = Proxy::new(&connection, owner.as_str(), path.as_str(), MENU_IFACE)
                        .await?;
                    // A menu owner can reject an id after it regenerated its
                    // layout. The refreshed rows below tell the truth about
                    // the menu, so the rejection stays silent instead of
                    // toasting an error the user cannot act on.
                    let _: std::result::Result<(), _> = proxy
                        .call(
                            "Event",
                            &(menu_id, "clicked", Value::from(0i32), now_ms() as u32),
                        )
                        .await;
                    // A toggled row repaints in the open popup: most menu
                    // owners restate their layout only around the next
                    // AboutToShow, so refetch right away and hand the new
                    // state to the shell.
                    let menu = get_menu(&connection, item).await?;
                    item.menu = menu.clone();
                    let _ = events.try_send(Event::TrayMenu { id, items: menu });
                }
                _ => {}
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = events.try_send(Event::Error(format!("桌面操作：{error:#}")));
        }
        if shutdown {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_dbusmenu_values_keep_state_and_bound_recursion() {
        let mut props = Properties::new();
        props.insert(
            "label".into(),
            OwnedValue::try_from(Value::from("_Open")).unwrap(),
        );
        props.insert("enabled".into(), OwnedValue::from(false));
        let item = parse_layout((7, props, vec![]), 0, &mut 8).unwrap();
        assert_eq!(item.label, "Open");
        assert!(!item.enabled);
        assert!(parse_layout((0, Properties::new(), vec![]), 9, &mut 8).is_none());
    }
    #[test]
    fn malformed_icon_dimensions_are_rejected() {
        assert!(icon(&Properties::new(), "IconPixmap").is_none());
    }
}
