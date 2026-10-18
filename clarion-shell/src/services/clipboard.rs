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

use super::history::{History, Payload, IMAGE_LIMIT, TEXT_LIMIT};
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
        if payload.mime == "text/plain" {
            source.offer("text/plain;charset=utf-8".into());
            source.offer("UTF8_STRING".into());
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
fn canonical(mime: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().replace([' ', '\t'], "").as_str() {
        "image/png" => Some("image/png"),
        "text/plain" | "text/plain;charset=utf-8" | "utf8_string" => Some("text/plain"),
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
                let selected = offered
                    .mimes
                    .iter()
                    .find(|mime| canonical(mime) == Some("image/png"))
                    .or_else(|| {
                        offered
                            .mimes
                            .iter()
                            .find(|mime| canonical(mime) == Some("text/plain"))
                    });
                let Some(mime) = selected else {
                    return;
                };
                let canonical = canonical(mime).unwrap().to_owned();
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
                std::thread::spawn(move || {
                    if let Some(bytes) = read_pipe(
                        reader,
                        if canonical == "image/png" {
                            IMAGE_LIMIT
                        } else {
                            TEXT_LIMIT
                        },
                        &epoch,
                        generation,
                    ) {
                        let _ = sender.send(ReadResult {
                            generation,
                            payload: Payload {
                                mime: canonical,
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
            source::Event::Send { mime_type, fd }
                if canonical(&mime_type) == Some(payload.mime.as_str()) =>
            {
                if state.write_jobs.load(Ordering::Acquire) >= 8 {
                    return;
                }
                let bytes = payload.bytes.clone();
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
        history: History::new(),
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
        assert_eq!(canonical("text/plain; charset=UTF-8"), Some("text/plain"));
        assert_eq!(canonical("text/html"), None);
    }
}
