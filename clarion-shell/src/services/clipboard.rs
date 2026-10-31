use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as device, ext_data_control_manager_v1 as manager,
    ext_data_control_offer_v1 as offer, ext_data_control_source_v1 as source,
};

use super::history::{History, Payload, BLOB_LIMIT, IMAGE_LIMIT, TEXT_LIMIT};
use super::{Command, Event};

const SCREENSHOT: &str = "application/x-clarion-screenshot";
#[derive(Default)]
struct OfferData {
    mimes: Vec<String>,
    sensitive: bool,
    overflow: bool,
}
struct ReadResult {
    generation: u64,
    payload: Payload,
}
struct State {
    events: Sender<Event>,
    history: History,
    manager: Option<manager::ExtDataControlManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    device: Option<device::ExtDataControlDeviceV1>,
    manager_name: u32,
    seat_name: u32,
    offers: HashMap<wayland_client::backend::ObjectId, (offer::ExtDataControlOfferV1, OfferData)>,
    selection: Option<offer::ExtDataControlOfferV1>,
    owned: HashMap<wayland_client::backend::ObjectId, source::ExtDataControlSourceV1>,
    generation: u64,
    epoch: Arc<AtomicU64>,
    read_jobs: Arc<AtomicUsize>,
    write_jobs: Arc<AtomicUsize>,
    reads: mpsc::Sender<ReadResult>,
}

impl Drop for State {
    fn drop(&mut self) {
        // I/O failures must invalidate both pending captures and the UI's
        // availability state, just like an orderly service shutdown.
        self.bump();
        let _ = self.events.try_send(Event::History {
            entries: self.history.snapshot(),
            paused: self.history.paused,
            available: false,
        });
    }
}

impl State {
    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.epoch.store(self.generation, Ordering::Release);
    }
    fn snapshot(&self) {
        let _ = self.events.try_send(Event::History {
            entries: self.history.snapshot(),
            paused: self.history.paused,
            available: self.device.is_some(),
        });
    }
    fn connect(&mut self, qh: &QueueHandle<Self>) {
        if self.device.is_none() {
            if let (Some(manager), Some(seat)) = (&self.manager, &self.seat) {
                self.device = Some(manager.get_data_device(seat, qh, ()));
                self.snapshot();
            }
        }
    }
    fn publish(&mut self, payload: Payload, qh: &QueueHandle<Self>) -> Result<()> {
        let device = self.device.clone().context("剪贴板协议当前不可用")?;
        let manager = self.manager.clone().context("剪贴板协议当前不可用")?;
        anyhow::ensure!(self.owned.len() < 8, "剪贴板发布正在等待确认");
        self.bump();
        let source = manager.create_data_source(qh, payload.clone());
        source.offer(payload.mime.clone());
        match payload.mime.as_str() {
            "text/plain" => {
                source.offer("text/plain;charset=utf-8".into());
                source.offer("UTF8_STRING".into());
            }
            // A file list is also useful as paths in a terminal or text field,
            // and rich text as its readable words.
            "text/uri-list" | "text/html" => {
                source.offer("text/plain".into());
                source.offer("text/plain;charset=utf-8".into());
            }
            _ => {}
        }
        source.offer(format!(
            "application/x-clarion-native-owner-{}",
            std::process::id()
        ));
        if payload.screenshot {
            source.offer(SCREENSHOT.into());
        }
        device.set_selection(Some(&source));
        self.owned.insert(source.id(), source);
        Ok(())
    }
    fn command(&mut self, command: Command, qh: &QueueHandle<Self>) -> bool {
        match command {
            Command::Shutdown => {
                self.bump();
                return false;
            }
            Command::Copy(id) => {
                if let Some(payload) = self.history.payload(&id) {
                    if let Err(error) = self.publish(payload, qh) {
                        let _ = self.events.try_send(Event::Error(error.to_string()));
                    }
                }
            }
            Command::Pin(id) => {
                self.history.pin(&id);
                self.snapshot();
            }
            Command::Delete(id) => {
                self.bump();
                self.history.delete(&id);
                self.snapshot();
            }
            Command::Clear => {
                self.bump();
                self.history.clear();
                self.snapshot();
            }
            Command::Pause(value) => {
                self.bump();
                self.history.paused = value;
                self.snapshot();
            }
            Command::RecordScreenshot(path)
                if !self.history.paused && std::path::Path::new(&path).is_absolute() =>
            {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                    .open(&path)
                {
                    if file
                        .metadata()
                        .is_ok_and(|m| m.is_file() && m.len() <= IMAGE_LIMIT as u64)
                    {
                        let mut bytes = Vec::new();
                        if (&mut file)
                            .take(IMAGE_LIMIT as u64 + 1)
                            .read_to_end(&mut bytes)
                            .is_ok()
                            && self.history.insert(Payload {
                                mime: "image/png".into(),
                                bytes: Arc::new(bytes),
                                screenshot: true,
                                file_path: Some(path),
                            })
                        {
                            self.snapshot();
                        }
                    }
                }
            }
            _ => {}
        }
        true
    }
}

pub(super) fn sensitive(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    [
        "password",
        "secret",
        "sensitive",
        "credential",
        "keepass",
        "klipper-ignore",
        "clipboard-manager-ignore",
    ]
    .iter()
    .any(|part| mime.contains(part))
}
/// The mime without parameters, lower-cased, with the legacy X11 atoms folded
/// into what they mean. `None` marks the atoms the selection protocol uses for
/// its own bookkeeping (`TARGETS`, timestamps, our own owner mark).
fn canonical(mime: &str) -> Option<String> {
    let lowered = mime.to_ascii_lowercase().replace([' ', '\t'], "");
    let base = lowered.split(';').next().unwrap_or_default();
    if base.is_empty() || base.starts_with("application/x-clarion-") {
        return None;
    }
    Some(
        match base {
            "utf8_string" | "string" | "text" | "compound_text" => "text/plain",
            "targets" | "timestamp" | "multiple" | "save_targets" | "delete"
            | "insert_selection" | "insert_property" => return None,
            other => other,
        }
        .to_owned(),
    )
}

/// Which offering to read, richest first: a still image, then rich text (its
/// plain rendering is rebuilt from the markup), a local file list, plain text,
/// any other textual flavor, and finally an opaque payload (PDF, video, audio)
/// that is stored and pasted back byte for byte.
fn pick_mime(offered: &[String]) -> Option<String> {
    let candidates: Vec<String> = offered.iter().filter_map(|mime| canonical(mime)).collect();
    for wanted in [
        "image/png",
        "image/jpeg",
        "text/html",
        "text/uri-list",
        "text/plain",
    ] {
        if let Some(mime) = candidates.iter().find(|mime| mime.as_str() == wanted) {
            return Some(mime.clone());
        }
    }
    if let Some(mime) = candidates.iter().find(|mime| !mime.starts_with("image/")) {
        return Some(mime.clone());
    }
    candidates.into_iter().next()
}

/// The bytes to hand a consumer asking for `mime` out of this payload. A file
/// list and a rich-text fragment also answer as plain text, so terminals and
/// text fields get the paths and the readable words.
fn payload_bytes(payload: &Payload, mime: &str) -> Option<Arc<Vec<u8>>> {
    let wanted = canonical(mime)?;
    if canonical(&payload.mime).as_deref() == Some(wanted.as_str()) {
        return Some(payload.bytes.clone());
    }
    match (payload.mime.as_str(), wanted.as_str()) {
        ("text/uri-list", "text/plain") => {
            let list = std::str::from_utf8(&payload.bytes).ok()?;
            let text = super::history::uri_paths(list)?
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\n");
            Some(Arc::new(text.into_bytes()))
        }
        ("text/html", "text/plain") => {
            let html = String::from_utf8_lossy(&payload.bytes);
            Some(Arc::new(
                super::history::html_to_text(&html).into_bytes(),
            ))
        }
        _ => None,
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface == manager::ExtDataControlManagerV1::interface().name
                    && state.manager.is_none()
                {
                    state.manager_name = name;
                    state.manager = Some(registry.bind(name, 1, qh, ()));
                }
                if interface == wl_seat::WlSeat::interface().name && state.seat.is_none() {
                    state.seat_name = name;
                    state.seat = Some(registry.bind(name, version.min(5), qh, ()));
                }
                state.connect(qh);
            }
            wl_registry::Event::GlobalRemove { name }
                if name == state.manager_name || name == state.seat_name =>
            {
                state.bump();
                if let Some(device) = state.device.take() {
                    device.destroy();
                }
                for (_, (offer, _)) in state.offers.drain() {
                    offer.destroy();
                }
                state.selection = None;
                if name == state.manager_name {
                    if let Some(manager) = state.manager.take() {
                        manager.destroy();
                    }
                }
                if name == state.seat_name {
                    if let Some(seat) = state.seat.take() {
                        if seat.version() >= 5 {
                            seat.release();
                        }
                    }
                }
                state.snapshot();
            }
            _ => {}
        }
    }
}
delegate_noop!(State:ignore wl_seat::WlSeat);
delegate_noop!(State:ignore manager::ExtDataControlManagerV1);

impl Dispatch<device::ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &device::ExtDataControlDeviceV1,
        event: device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            device::Event::DataOffer { id } => {
                state.offers.insert(id.id(), (id, OfferData::default()));
            }
            device::Event::PrimarySelection { id: Some(id) } => {
                if let Some((offer, _)) = state.offers.remove(&id.id()) {
                    offer.destroy();
                }
            }
            device::Event::Selection { id } => {
                state.bump();
                if let Some(previous) = state.selection.take() {
                    if let Some((offer, _)) = state.offers.remove(&previous.id()) {
                        offer.destroy();
                    }
                }
                let Some(proxy) = id else {
                    return;
                };
                state.selection = Some(proxy.clone());
                let Some((_, offered)) = state.offers.get(&proxy.id()) else {
                    return;
                };
                if state.history.paused
                    || offered.sensitive
                    || offered.overflow
                    || offered.mimes.iter().any(|m| {
                        m == &format!("application/x-clarion-native-owner-{}", std::process::id())
                    })
                {
                    return;
                }
                let Some(mime) = pick_mime(&offered.mimes) else {
                    return;
                };
                let screenshot = offered.mimes.iter().any(|m| m == SCREENSHOT);
                if state.read_jobs.load(Ordering::Acquire) >= 2 {
                    return;
                }
                let mut descriptors = [0; 2];
                if unsafe {
                    libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK)
                } != 0
                {
                    return;
                }
                let reader = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
                let writer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
                // A producer may use blocking writes; leave only our reader nonblocking.
                unsafe {
                    libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, 0);
                }
                proxy.receive(mime.clone(), writer.as_fd());
                drop(writer);
                let generation = state.generation;
                let sender = state.reads.clone();
                let epoch = state.epoch.clone();
                let jobs = state.read_jobs.clone();
                jobs.fetch_add(1, Ordering::AcqRel);
                let limit = if mime.starts_with("image/") {
                    IMAGE_LIMIT
                } else if mime.starts_with("text/") {
                    TEXT_LIMIT
                } else {
                    BLOB_LIMIT
                };
                std::thread::spawn(move || {
                    if let Some(bytes) = read_pipe(reader, limit, &epoch, generation) {
                        let _ = sender.send(ReadResult {
                            generation,
                            payload: Payload {
                                mime,
                                bytes: Arc::new(bytes),
                                screenshot,
                                file_path: None,
                            },
                        });
                    }
                    jobs.fetch_sub(1, Ordering::AcqRel);
                });
            }
            device::Event::Finished => {
                state.bump();
                if let Some(device) = state.device.take() {
                    device.destroy();
                }
                state.snapshot();
            }
            _ => {}
        }
    }
    event_created_child!(State,device::ExtDataControlDeviceV1,[device::EVT_DATA_OFFER_OPCODE=>(offer::ExtDataControlOfferV1,())]);
}
impl Dispatch<offer::ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &offer::ExtDataControlOfferV1,
        event: offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let offer::Event::Offer { mime_type } = event {
            if let Some((_, offer)) = state.offers.get_mut(&proxy.id()) {
                offer.sensitive |= sensitive(&mime_type);
                if mime_type.len() > 256 || offer.mimes.len() >= 128 {
                    offer.overflow = true;
                } else {
                    offer.mimes.push(mime_type);
                }
            }
        }
    }
}
impl Dispatch<source::ExtDataControlSourceV1, Payload> for State {
    fn event(
        state: &mut Self,
        proxy: &source::ExtDataControlSourceV1,
        event: source::Event,
        payload: &Payload,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            source::Event::Send { mime_type, fd } => {
                let Some(bytes) = payload_bytes(payload, &mime_type) else {
                    return;
                };
                if state.write_jobs.load(Ordering::Acquire) >= 8 {
                    return;
                }
                let jobs = state.write_jobs.clone();
                jobs.fetch_add(1, Ordering::AcqRel);
                std::thread::spawn(move || {
                    write_pipe(fd, bytes);
                    jobs.fetch_sub(1, Ordering::AcqRel);
                });
            }
            source::Event::Cancelled => {
                proxy.destroy();
                state.owned.remove(&proxy.id());
            }
            _ => {}
        }
    }
}

fn read_pipe(fd: OwnedFd, limit: usize, epoch: &AtomicU64, generation: u64) -> Option<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut file = File::from(fd);
    let mut bytes = Vec::new();
    let mut buffer = [0; 65536];
    while Instant::now() < deadline && epoch.load(Ordering::Acquire) == generation {
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 50) } < 0 {
            continue;
        }
        match file.read(&mut buffer) {
            Ok(0) => return Some(bytes),
            Ok(count) => {
                if count > limit.saturating_sub(bytes.len()) {
                    return None;
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return None,
        }
    }
    None
}
fn write_pipe(fd: OwnedFd, bytes: Arc<Vec<u8>>) {
    // Ignore SIGPIPE in this thread only; do not alter GPUI's process handlers.
    unsafe {
        let mut set = std::mem::zeroed();
        let mut old = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGPIPE);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old) != 0 {
            return;
        }
        let mut pending = std::mem::zeroed();
        libc::sigpending(&mut pending);
        let had_pending = libc::sigismember(&pending, libc::SIGPIPE) == 1;
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        let mut file = File::from(fd);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut offset = 0;
        while offset < bytes.len() && Instant::now() < deadline {
            let mut poll = libc::pollfd {
                fd: file.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            libc::poll(&mut poll, 1, 50);
            match file.write(&bytes[offset..]) {
                Ok(0) => break,
                Ok(count) => offset += count,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => break,
            }
        }
        if !had_pending && libc::sigismember(&old, libc::SIGPIPE) != 1 {
            let zero = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            libc::sigtimedwait(&set, std::ptr::null_mut(), &zero);
        }
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
    }
}

pub(super) fn run(events: Sender<Event>, commands: Receiver<Command>) -> Result<()> {
    let connection = Connection::connect_to_env().context("无法连接Wayland")?;
    let mut queue = connection.new_event_queue::<State>();
    let qh = queue.handle();
    let registry = connection.display().get_registry(&qh, ());
    let (reads, completed) = mpsc::channel();
    let mut state = State {
        events,
        history: History::with_state(super::history::state_dir()),
        manager: None,
        seat: None,
        device: None,
        manager_name: 0,
        seat_name: 0,
        offers: HashMap::new(),
        selection: None,
        owned: HashMap::new(),
        generation: 0,
        epoch: Arc::new(AtomicU64::new(0)),
        read_jobs: Arc::new(AtomicUsize::new(0)),
        write_jobs: Arc::new(AtomicUsize::new(0)),
        reads,
    };
    state.snapshot();
    loop {
        while let Ok(command) = commands.try_recv() {
            if !state.command(command, &qh) {
                drop(registry);
                return Ok(());
            }
        }
        while let Ok(read) = completed.try_recv() {
            if read.generation == state.generation
                && !state.history.paused
                && state.history.insert(read.payload)
            {
                state.snapshot();
            }
        }
        queue.dispatch_pending(&mut state)?;
        connection.flush()?;
        if let Some(guard) = queue.prepare_read() {
            let mut poll = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 15) } > 0 {
                guard.read()?;
            } else {
                drop(guard);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mime_filter_and_limits() {
        assert!(sensitive("x-kde-passwordManagerHint"));
        assert!(sensitive("application/x-keepassxc"));
        assert!(!sensitive("text/plain"));
        assert_eq!(
            canonical("text/plain; charset=UTF-8").as_deref(),
            Some("text/plain")
        );
        assert_eq!(canonical("text/uri-list").as_deref(), Some("text/uri-list"));
        assert_eq!(canonical("text/html").as_deref(), Some("text/html"));
        assert_eq!(canonical("UTF8_STRING").as_deref(), Some("text/plain"));
        assert_eq!(
            canonical("application/x-clarion-native-owner-42"),
            None,
            "our own owner mark is not content"
        );
        assert_eq!(canonical("TARGETS"), None, "protocol bookkeeping");
    }

    #[test]
    fn the_richest_offering_wins() {
        let offer = |mimes: &[&str]| mimes.iter().map(|mime| (*mime).to_owned()).collect::<Vec<_>>();
        // Rich text is preferred over its own plain rendering.
        assert_eq!(
            pick_mime(&offer(&[
                "text/plain;charset=utf-8",
                "text/html;charset=utf-8",
                "TARGETS",
            ]))
            .as_deref(),
            Some("text/html")
        );
        // A file list beats plain text; a still image beats everything.
        assert_eq!(
            pick_mime(&offer(&["text/plain", "text/uri-list"])).as_deref(),
            Some("text/uri-list")
        );
        assert_eq!(
            pick_mime(&offer(&["text/html", "image/png", "text/plain"])).as_deref(),
            Some("image/png")
        );
        // Opaque payloads are still captured, so a video or a PDF copy lands in
        // the history instead of being dropped.
        assert_eq!(
            pick_mime(&offer(&["video/mp4", "audio/x-wav"])).as_deref(),
            Some("video/mp4")
        );
        assert_eq!(
            pick_mime(&offer(&["TARGETS", "TIMESTAMP", "text/plain;charset=utf-8"])).as_deref(),
            Some("text/plain")
        );
        assert_eq!(pick_mime(&[]), None);
    }

    #[test]
    fn rich_text_also_answers_as_plain_text() {
        let payload = Payload {
            mime: "text/html".into(),
            bytes: Arc::new(
                b"<p>Hello <b>world</b></p><p>second &amp; last</p>".to_vec(),
            ),
            screenshot: false,
            file_path: None,
        };
        assert_eq!(
            payload_bytes(&payload, "text/plain")
                .expect("the readable words")
                .as_slice(),
            b"Hello world\nsecond & last"
        );
        assert_eq!(
            payload_bytes(&payload, "text/html")
                .expect("the original markup")
                .as_slice(),
            payload.bytes.as_slice()
        );
        assert!(payload_bytes(&payload, "text/uri-list").is_none());
    }

    #[test]
    fn file_lists_also_publish_their_paths_as_text() {
        let payload = Payload {
            mime: "text/uri-list".into(),
            bytes: Arc::new(
                b"file:///tmp/one%20two.txt\r\n# a comment\r\nfile://localhost/tmp/three.png\r\n"
                    .to_vec(),
            ),
            screenshot: false,
            file_path: None,
        };
        let bytes = payload_bytes(&payload, "text/plain").expect("paths for text consumers");
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "/tmp/one two.txt\n/tmp/three.png"
        );
        assert_eq!(
            payload_bytes(&payload, "text/uri-list")
                .expect("the original list")
                .as_slice(),
            payload.bytes.as_slice()
        );
        assert!(payload_bytes(&payload, "image/png").is_none());
    }
}
