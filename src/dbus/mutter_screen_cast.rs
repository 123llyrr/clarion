use std::collections::HashMap;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::Deserialize;
use zbus::fdo::RequestNameFlags;
use zbus::object_server::{InterfaceRef, SignalEmitter};
use zbus::zvariant::{DeserializeDict, OwnedObjectPath, SerializeDict, Type, Value};
use zbus::{fdo, interface, ObjectServer};

use super::Start;
use crate::backend::IpcOutputMap;
use crate::utils::{CastSessionId, CastStreamId};

type SessionRegistry = Mutex<Vec<(Session, InterfaceRef<Session>)>>;

#[derive(Clone)]
pub struct ScreenCast {
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    to_realand: calloop::channel::Sender<ScreenCastToRealand>,
    sessions: Arc<SessionRegistry>,
}

#[derive(Clone)]
pub struct Session {
    id: CastSessionId,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    to_realand: calloop::channel::Sender<ScreenCastToRealand>,
    #[allow(clippy::type_complexity)]
    streams: Arc<Mutex<Vec<(Stream, InterfaceRef<Stream>)>>>,
    stopped: Arc<AtomicBool>,
    sessions: Weak<SessionRegistry>,
}

#[derive(Debug, Default, Deserialize, Type, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    #[default]
    Hidden = 0,
    Embedded = 1,
    Metadata = 2,
}

#[derive(Debug, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct RecordMonitorProperties {
    #[zvariant(rename = "cursor-mode")]
    cursor_mode: Option<CursorMode>,
    #[zvariant(rename = "is-recording")]
    _is_recording: Option<bool>,
}

#[derive(Debug, DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct RecordWindowProperties {
    #[zvariant(rename = "window-id")]
    window_id: u64,
    #[zvariant(rename = "cursor-mode")]
    cursor_mode: Option<CursorMode>,
    #[zvariant(rename = "is-recording")]
    _is_recording: Option<bool>,
}

#[derive(Clone)]
pub struct Stream {
    id: CastStreamId,
    session_id: CastSessionId,
    target: StreamTarget,
    cursor_mode: CursorMode,
    was_started: Arc<AtomicBool>,
    to_realand: calloop::channel::Sender<ScreenCastToRealand>,
}

#[derive(Clone)]
enum StreamTarget {
    // FIXME: update on scale changes and whatnot.
    Output(realand_ipc::Output),
    Window { id: u64 },
}

#[derive(Debug, Clone)]
pub enum StreamTargetId {
    Output { name: String },
    Window { id: u64 },
}

#[derive(Debug, SerializeDict, Type, Value)]
#[zvariant(signature = "dict")]
struct StreamParameters {
    /// Position of the stream in logical coordinates.
    position: (i32, i32),
    /// Size of the stream in logical coordinates.
    size: (i32, i32),
}

pub enum ScreenCastToRealand {
    StartCast {
        session_id: CastSessionId,
        stream_id: CastStreamId,
        target: StreamTargetId,
        cursor_mode: CursorMode,
        signal_ctx: SignalEmitter<'static>,
    },
    StopCast {
        session_id: CastSessionId,
    },
}

#[interface(name = "org.gnome.Mutter.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        properties: HashMap<&str, Value<'_>>,
    ) -> fdo::Result<OwnedObjectPath> {
        if properties.contains_key("remote-desktop-session-id") {
            return Err(fdo::Error::Failed(
                "there are no remote desktop sessions".to_owned(),
            ));
        }

        let session_id = CastSessionId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Session/u{}", session_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let session = Session::new(
            session_id,
            self.ipc_outputs.clone(),
            self.to_realand.clone(),
            Arc::downgrade(&self.sessions),
        );
        match server.at(&path, session.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.sessions.lock().unwrap().push((session, iface));
            }
            Ok(false) => return Err(fdo::Error::Failed("session path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating session object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    #[zbus(property)]
    async fn version(&self) -> i32 {
        4
    }
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Session")]
impl Session {
    async fn start(&self) {
        debug!("start");

        if self.stopped.load(Ordering::SeqCst) {
            return;
        }

        for (stream, iface) in &*self.streams.lock().unwrap() {
            stream.start(iface.signal_emitter().clone());
        }
    }

    pub async fn stop(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
    ) {
        debug!("stop");

        if self.stopped.swap(true, Ordering::SeqCst) {
            // Already stopped.
            return;
        }

        Session::closed(&ctxt).await.unwrap();

        if let Err(err) = self.to_realand.send(ScreenCastToRealand::StopCast {
            session_id: self.id,
        }) {
            warn!("error sending StopCast to realand: {err:?}");
        }

        let streams = mem::take(&mut *self.streams.lock().unwrap());
        for (_, iface) in streams.iter() {
            server
                .remove::<Stream, _>(iface.signal_emitter().path())
                .await
                .unwrap();
        }

        server.remove::<Session, _>(ctxt.path()).await.unwrap();
        if let Some(sessions) = self.sessions.upgrade() {
            sessions
                .lock()
                .unwrap()
                .retain(|(session, _)| session.id != self.id);
        }
    }

    async fn record_monitor(
        &mut self,
        #[zbus(object_server)] server: &ObjectServer,
        connector: &str,
        properties: RecordMonitorProperties,
    ) -> fdo::Result<OwnedObjectPath> {
        debug!(connector, ?properties, "record_monitor");

        let output = {
            let ipc_outputs = self.ipc_outputs.lock().unwrap();
            ipc_outputs.values().find(|o| o.name == connector).cloned()
        };
        let Some(output) = output else {
            return Err(fdo::Error::Failed("no such monitor".to_owned()));
        };

        if output.logical.is_none() {
            return Err(fdo::Error::Failed("monitor is disabled".to_owned()));
        }

        let stream_id = CastStreamId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Stream/u{}", stream_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let cursor_mode = properties.cursor_mode.unwrap_or_default();

        let target = StreamTarget::Output(output);
        let stream = Stream::new(
            stream_id,
            self.id,
            target,
            cursor_mode,
            self.to_realand.clone(),
        );
        match server.at(&path, stream.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.streams.lock().unwrap().push((stream, iface));
            }
            Ok(false) => return Err(fdo::Error::Failed("stream path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating stream object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    async fn record_window(
        &mut self,
        #[zbus(object_server)] server: &ObjectServer,
        properties: RecordWindowProperties,
    ) -> fdo::Result<OwnedObjectPath> {
        debug!(?properties, "record_window");

        let stream_id = CastStreamId::next();
        let path = format!("/org/gnome/Mutter/ScreenCast/Stream/u{}", stream_id.get());
        let path = OwnedObjectPath::try_from(path).unwrap();

        let cursor_mode = properties.cursor_mode.unwrap_or_default();

        let target = StreamTarget::Window {
            id: properties.window_id,
        };
        let stream = Stream::new(
            stream_id,
            self.id,
            target,
            cursor_mode,
            self.to_realand.clone(),
        );
        match server.at(&path, stream.clone()).await {
            Ok(true) => {
                let iface = server.interface(&path).await.unwrap();
                self.streams.lock().unwrap().push((stream, iface));
            }
            Ok(false) => return Err(fdo::Error::Failed("stream path already exists".to_owned())),
            Err(err) => {
                return Err(fdo::Error::Failed(format!(
                    "error creating stream object: {err:?}"
                )))
            }
        }

        Ok(path)
    }

    #[zbus(signal)]
    async fn closed(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Stream")]
impl Stream {
    #[zbus(signal)]
    pub async fn pipe_wire_stream_added(ctxt: &SignalEmitter<'_>, node_id: u32)
        -> zbus::Result<()>;

    #[zbus(property)]
    async fn parameters(&self) -> StreamParameters {
        match &self.target {
            StreamTarget::Output(output) => {
                let logical = output.logical.as_ref().unwrap();
                StreamParameters {
                    position: (logical.x, logical.y),
                    size: (logical.width as i32, logical.height as i32),
                }
            }
            StreamTarget::Window { .. } => {
                // Does any consumer need this?
                StreamParameters {
                    position: (0, 0),
                    size: (1, 1),
                }
            }
        }
    }
}

impl ScreenCast {
    pub fn new(
        ipc_outputs: Arc<Mutex<IpcOutputMap>>,
        to_realand: calloop::channel::Sender<ScreenCastToRealand>,
    ) -> Self {
        Self {
            ipc_outputs,
            to_realand,
            sessions: Arc::new(Mutex::new(vec![])),
        }
    }
}

impl Start for ScreenCast {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server()
            .at("/org/gnome/Mutter/ScreenCast", self)?;
        conn.request_name_with_flags("org.gnome.Mutter.ScreenCast", flags)?;

        Ok(conn)
    }
}

impl Session {
    fn new(
        id: CastSessionId,
        ipc_outputs: Arc<Mutex<IpcOutputMap>>,
        to_realand: calloop::channel::Sender<ScreenCastToRealand>,
        sessions: Weak<SessionRegistry>,
    ) -> Self {
        Self {
            id,
            ipc_outputs,
            streams: Arc::new(Mutex::new(vec![])),
            to_realand,
            stopped: Arc::new(AtomicBool::new(false)),
            sessions,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.to_realand.send(ScreenCastToRealand::StopCast {
            session_id: self.id,
        });
    }
}

impl Stream {
    fn new(
        id: CastStreamId,
        session_id: CastSessionId,
        target: StreamTarget,
        cursor_mode: CursorMode,
        to_realand: calloop::channel::Sender<ScreenCastToRealand>,
    ) -> Self {
        Self {
            id,
            session_id,
            target,
            cursor_mode,
            was_started: Arc::new(AtomicBool::new(false)),
            to_realand,
        }
    }

    fn start(&self, ctxt: SignalEmitter<'static>) {
        if self.was_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let msg = ScreenCastToRealand::StartCast {
            session_id: self.session_id,
            stream_id: self.id,
            target: self.target.make_id(),
            cursor_mode: self.cursor_mode,
            signal_ctx: ctxt,
        };

        if let Err(err) = self.to_realand.send(msg) {
            self.was_started.store(false, Ordering::SeqCst);
            warn!("error sending StartCast to realand: {err:?}");
        }
    }
}

impl StreamTarget {
    fn make_id(&self) -> StreamTargetId {
        match self {
            StreamTarget::Output(output) => StreamTargetId::Output {
                name: output.name.clone(),
            },
            StreamTarget::Window { id } => StreamTargetId::Window { id: *id },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use super::*;

    // Use an authenticated in-process socket so these tests need neither a
    // session bus nor a PipeWire server or GPU.
    fn test_connection() -> (zbus::Connection, UnixStream) {
        let (socket, peer) = UnixStream::pair().unwrap();
        let connection = async_io::block_on(
            zbus::connection::Builder::authenticated_socket(
                async_io::Async::new(socket).unwrap(),
                "0123456789abcdef0123456789abcdef",
            )
            .unwrap()
            .internal_executor(false)
            .build(),
        )
        .unwrap();
        (connection, peer)
    }

    #[test]
    fn repeated_stream_start_enqueues_only_one_cast() {
        let (connection, _peer) = test_connection();
        let ctxt =
            SignalEmitter::new(&connection, "/org/gnome/Mutter/ScreenCast/Stream/test").unwrap();
        let (sender, receiver) = calloop::channel::channel();
        let stream = Stream::new(
            CastStreamId::next(),
            CastSessionId::next(),
            StreamTarget::Window { id: 1 },
            CursorMode::Hidden,
            sender,
        );
        stream.start(ctxt.clone());
        stream.clone().start(ctxt);
        assert!(matches!(
            receiver.try_recv(),
            Ok(ScreenCastToRealand::StartCast { .. })
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn stopped_session_is_removed_from_registry() {
        let (connection, _peer) = test_connection();
        let (sender, _receiver) = calloop::channel::channel();
        let screen_cast = ScreenCast::new(Arc::new(Mutex::new(HashMap::new())), sender);
        async_io::block_on(async {
            let server = connection.object_server();
            let path = screen_cast
                .create_session(server, HashMap::new())
                .await
                .unwrap();
            assert_eq!(screen_cast.sessions.lock().unwrap().len(), 1);
            let iface = server.interface::<_, Session>(&path).await.unwrap();
            iface
                .get()
                .await
                .stop(server, iface.signal_emitter().clone())
                .await;
            assert!(screen_cast.sessions.lock().unwrap().is_empty());
            assert!(server.interface::<_, Session>(&path).await.is_err());
        });
    }
}
