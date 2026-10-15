use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::{env, io, process};

use anyhow::Context;
use async_channel::{Receiver, Sender, TrySendError};
use calloop::futures::Scheduler;
use calloop::io::Async;
use directories::BaseDirs;
use futures_util::io::{AsyncReadExt, BufReader};
use futures_util::{select_biased, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, FutureExt as _};
use realand_config::OutputName;
use realand_ipc::state::{EventStreamState, EventStreamStatePart as _};
use realand_ipc::{
    Action, Event, KeyboardLayouts, OutputConfigChanged, Overview, Reply, Request, Response,
    Timestamp, WindowLayout, Workspace,
};
use smithay::desktop::layer_map_for_output;
use smithay::input::pointer::{
    CursorIcon, CursorImageStatus, Focus, GrabStartData as PointerGrabStartData,
};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::rustix::fs::unlink;
use smithay::reexports::rustix::net::sockopt::socket_peercred;
use smithay::reexports::rustix::process::geteuid;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer};

use crate::backend::IpcOutputMap;
use crate::compositor::State;
use crate::input::pick_window_grab::PickWindowGrab;
use crate::layout::workspace::WorkspaceId;
use crate::utils::{version, with_toplevel_role};
use crate::window::Mapped;

// If an event stream client fails to read events fast enough that we accumulate more than this
// number in our buffer, we drop that event stream client.
const EVENT_STREAM_BUFFER_SIZE: usize = 64;
// A saved session is capped at 4 MiB by settings; allow JSON escaping overhead.
// Keep the same bound for every request, reply, and event line so both sides
// enforce one predictable protocol limit.
const MAX_IPC_LINE_BYTES: usize = realand_ipc::socket::MAX_LINE_BYTES;

pub struct IpcServer {
    /// Path to the IPC socket.
    ///
    /// This is `None` when creating `IpcServer` without a socket.
    pub socket_path: Option<PathBuf>,
    event_streams: Rc<RefCell<Vec<EventStreamSender>>>,
    event_stream_state: Rc<RefCell<EventStreamState>>,
}

struct ClientCtx {
    owned_tasks: crate::utils::owned_tasks::TaskPool,
    event_loop: LoopHandle<'static, State>,
    scheduler: Scheduler<()>,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    event_streams: Rc<RefCell<Vec<EventStreamSender>>>,
    event_stream_state: Rc<RefCell<EventStreamState>>,
}

struct EventStreamClient {
    events: Receiver<Event>,
    disconnect: Receiver<()>,
    write: Box<dyn AsyncWrite + Unpin>,
}

struct EventStreamSender {
    events: Sender<Event>,
    disconnect: Sender<()>,
}

impl IpcServer {
    pub fn start(
        event_loop: &LoopHandle<'static, State>,
        wayland_socket_name: Option<&OsStr>,
    ) -> anyhow::Result<Self> {
        let _span = realand_tracy_span!("Ipc::start");

        let socket_path = if let Some(wayland_socket_name) = wayland_socket_name {
            let wayland_socket_name = wayland_socket_name.to_string_lossy();
            let socket_name = format!("realand.{wayland_socket_name}.{}.sock", process::id());
            let mut socket_path = socket_dir();
            socket_path.push(socket_name);

            let listener = bind_private_socket(&socket_path)?;
            listener
                .set_nonblocking(true)
                .context("error setting socket to non-blocking")?;

            let source = Generic::new(listener, Interest::READ, Mode::Level);
            event_loop
                .insert_source(source, |_, socket, state| {
                    match socket.accept() {
                        Ok((stream, _)) => on_new_ipc_client(state, stream),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                        Err(e) => return Err(e),
                    }

                    Ok(PostAction::Continue)
                })
                .unwrap();

            Some(socket_path)
        } else {
            None
        };

        Ok(Self {
            socket_path,
            event_streams: Rc::new(RefCell::new(Vec::new())),
            event_stream_state: Rc::new(RefCell::new(EventStreamState::default())),
        })
    }

    fn send_event(&self, event: Event) {
        let mut streams = self.event_streams.borrow_mut();
        let mut to_remove = Vec::new();
        for (idx, stream) in streams.iter_mut().enumerate() {
            match stream.events.try_send(event.clone()) {
                Ok(()) => (),
                Err(TrySendError::Closed(_)) => to_remove.push(idx),
                Err(TrySendError::Full(_)) => {
                    warn!(
                        "disconnecting IPC event stream client \
                         because it is reading events too slowly"
                    );
                    to_remove.push(idx);
                }
            }
        }

        for idx in to_remove.into_iter().rev() {
            let stream = streams.swap_remove(idx);
            let _ = stream.disconnect.send_blocking(());
        }
    }

    #[cfg(test)]
    pub(crate) fn test_windows(&self) -> Vec<realand_ipc::Window> {
        self.event_stream_state
            .borrow()
            .windows
            .windows
            .values()
            .cloned()
            .collect()
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Some(socket_path) = &self.socket_path {
            let _ = unlink(socket_path);
        }
    }
}

fn socket_dir() -> PathBuf {
    BaseDirs::new()
        .as_ref()
        .and_then(|x| x.runtime_dir())
        .map(|x| x.to_owned())
        .unwrap_or_else(env::temp_dir)
}

/// Bind the IPC endpoint and restrict it to the compositor's Unix user.
///
/// The runtime directory is normally private, but the fallback to `/tmp` is
/// intentionally supported for unusual sessions. Explicitly setting the mode
/// keeps that fallback from turning the control socket into a cross-user API.
fn bind_private_socket(socket_path: &Path) -> anyhow::Result<UnixListener> {
    let listener = UnixListener::bind(socket_path).context("error binding socket")?;
    if let Err(err) = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)) {
        drop(listener);
        let _ = fs::remove_file(socket_path);
        return Err(err).context("error restricting IPC socket permissions");
    }
    Ok(listener)
}

fn on_new_ipc_client(state: &mut State, stream: UnixStream) {
    let _span = realand_tracy_span!("on_new_ipc_client");
    // A connection can be queued between bind() and chmod(), particularly
    // when the endpoint is in /tmp. Authenticate the accepted peer as well.
    if let Err(err) = check_ipc_peer(&stream) {
        warn!("rejecting IPC client: {err}");
        return;
    }
    trace!("new IPC client connected");

    let stream = match state.realand.event_loop.adapt_io(stream) {
        Ok(stream) => stream,
        Err(err) => {
            warn!("error making IPC stream async: {err:?}");
            return;
        }
    };

    let ipc_server = state.realand.ipc_server.as_ref().unwrap();

    let ctx = ClientCtx {
        owned_tasks: state.realand.owned_tasks.clone(),
        event_loop: state.realand.event_loop.clone(),
        scheduler: state.realand.scheduler.clone(),
        ipc_outputs: state.backend.ipc_outputs(),
        event_streams: ipc_server.event_streams.clone(),
        event_stream_state: ipc_server.event_stream_state.clone(),
    };

    let future = async move {
        if let Err(err) = handle_client(ctx, stream).await {
            warn!("error handling IPC client: {err:?}");
        }
    };
    if let Err(err) = state.realand.scheduler.schedule(future) {
        warn!("error scheduling IPC stream future: {err:?}");
    }
}

fn check_ipc_peer(stream: &UnixStream) -> io::Result<()> {
    if socket_peercred(stream)?.uid != geteuid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC client belongs to a different Unix user",
        ));
    }
    Ok(())
}

async fn handle_client(ctx: ClientCtx, stream: Async<'static, UnixStream>) -> anyhow::Result<()> {
    let (read, mut write) = stream.split();
    let mut read = BufReader::new(read);

    loop {
        // Bound a single request even when a client never sends a newline.
        let mut buf = match read_request(&mut read, MAX_IPC_LINE_BYTES).await {
            Ok(buf) if buf.is_empty() => return Ok(()),
            Ok(buf) => buf,
            // Normal client disconnection.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(err) => {
                return Err(err).context("error reading request");
            }
        };

        // Large saved-session payloads have an outer JSON string as well as inner JSON.
        // Small commands retain the immediate path and per-connection order is unchanged.
        let request = if buf.len() > 16 * 1024 {
            match ctx.owned_tasks.submit(move || {
                serde_json::from_slice::<Request>(&buf)
                    .map_err(|e| format!("error parsing request: {e}"))
            }) {
                Ok(task) => task.finish().await,
                Err(err) => Err(err),
            }
        } else {
            serde_json::from_slice::<Request>(&buf)
                .map_err(|e| format!("error parsing request: {e}"))
        };
        let requested_error = matches!(request, Ok(Request::ReturnError));
        let requested_event_stream = matches!(request, Ok(Request::EventStream));

        let reply = match request {
            Ok(request) => process(&ctx, request).await,
            Err(err) => Err(err),
        };

        if let Err(err) = &reply {
            if !requested_error {
                warn!("error processing IPC request: {err:?}");
            }
        }

        let large_reply = matches!(
            &reply,
            Ok(Response::ManagementState(_)) | Ok(Response::FrameTimings(_))
        ) || matches!(&reply, Ok(Response::Windows(windows)) if windows.len() > 128);
        buf = if large_reply {
            ctx.owned_tasks
                .submit(move || serialize_ipc_line(&reply, "reply").map_err(|e| e.to_string()))
                .map_err(anyhow::Error::msg)?
                .finish()
                .await
                .map_err(anyhow::Error::msg)?
        } else {
            serialize_ipc_line(&reply, "reply")?
        };
        write.write_all(&buf).await.context("error writing reply")?;

        if requested_event_stream {
            let (events_tx, events_rx) = async_channel::bounded(EVENT_STREAM_BUFFER_SIZE);
            let (disconnect_tx, disconnect_rx) = async_channel::bounded(1);

            // Spawn a task for the client.
            let client = EventStreamClient {
                events: events_rx,
                disconnect: disconnect_rx,
                write: Box::new(write) as _,
            };
            let future = async move {
                if let Err(err) = handle_event_stream_client(client).await {
                    warn!("error handling IPC event stream client: {err:?}");
                }
            };
            if let Err(err) = ctx.scheduler.schedule(future) {
                warn!("error scheduling IPC event stream future: {err:?}");
            }

            // Send the initial state.
            {
                let state = ctx.event_stream_state.borrow();
                for event in state.replicate() {
                    events_tx
                        .try_send(event)
                        .expect("initial event burst had more events than buffer size");
                }
            }

            // Add it to the list.
            {
                let mut streams = ctx.event_streams.borrow_mut();
                let sender = EventStreamSender {
                    events: events_tx,
                    disconnect: disconnect_tx,
                };
                streams.push(sender);
            }

            return Ok(());
        }
    }
}

async fn read_request(
    read: &mut (impl futures_util::io::AsyncBufRead + Unpin),
    limit: usize,
) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    read.take((limit + 1) as u64)
        .read_until(b'\n', &mut buf)
        .await?;
    if buf.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC request too large",
        ));
    }
    Ok(buf)
}

fn serialize_ipc_line<T: serde::Serialize>(value: &T, kind: &str) -> anyhow::Result<Vec<u8>> {
    let mut buf = serde_json::to_vec(value).with_context(|| format!("error formatting {kind}"))?;
    if buf.len() + 1 > MAX_IPC_LINE_BYTES {
        anyhow::bail!(
            "IPC {kind} too large: {} bytes (maximum {} bytes including newline)",
            buf.len() + 1,
            MAX_IPC_LINE_BYTES,
        );
    }
    buf.push(b'\n');
    Ok(buf)
}

async fn process(ctx: &ClientCtx, request: Request) -> Reply {
    let response = match request {
        Request::ReturnError => return Err(String::from("example compositor error")),
        Request::Version => Response::Version(version()),
        Request::Capabilities => {
            Response::Capabilities(vec!["desktop-menu-focus-restore".to_owned()])
        }
        Request::FrameTimings => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let _ = tx.send_blocking(state.realand.frame_timings());
            });
            Response::FrameTimings(rx.recv().await.map_err(|e| e.to_string())?)
        }
        Request::Outputs => {
            let ipc_outputs = ctx.ipc_outputs.lock().unwrap().clone();
            let outputs = ipc_outputs.values().cloned().map(|o| (o.name.clone(), o));
            Response::Outputs(outputs.collect())
        }
        Request::Workspaces => {
            let state = ctx.event_stream_state.borrow();
            let workspaces = state.workspaces.workspaces.values().cloned().collect();
            Response::Workspaces(workspaces)
        }
        Request::Windows => {
            let state = ctx.event_stream_state.borrow();
            let windows = state.windows.windows.values().cloned().collect();
            Response::Windows(windows)
        }
        Request::ManagementState => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let layout = &state.realand.layout;
                let (groups, undo_steps, redo_steps) = layout.management_status();
                let result = realand_ipc::management::ManagementState {
                    groups,
                    undo_steps,
                    redo_steps,
                    session: layout.session_layout(),
                };
                let _ = tx.try_send(result);
            });
            let snapshot = rx.recv().await.map_err(|e| e.to_string())?;
            let json = ctx
                .owned_tasks
                .submit(move || serde_json::to_string(&snapshot).map_err(|e| e.to_string()))?
                .finish()
                .await?;
            Response::ManagementState(json)
        }
        Request::Layers => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let mut layers = Vec::new();
                for output in state.realand.global_space.outputs() {
                    let name = output.name();
                    for surface in layer_map_for_output(output).layers() {
                        let layer = match surface.layer() {
                            Layer::Background => realand_ipc::Layer::Background,
                            Layer::Bottom => realand_ipc::Layer::Bottom,
                            Layer::Top => realand_ipc::Layer::Top,
                            Layer::Overlay => realand_ipc::Layer::Overlay,
                        };
                        let keyboard_interactivity =
                            match surface.cached_state().keyboard_interactivity {
                                KeyboardInteractivity::None => {
                                    realand_ipc::LayerSurfaceKeyboardInteractivity::None
                                }
                                KeyboardInteractivity::Exclusive => {
                                    realand_ipc::LayerSurfaceKeyboardInteractivity::Exclusive
                                }
                                KeyboardInteractivity::OnDemand => {
                                    realand_ipc::LayerSurfaceKeyboardInteractivity::OnDemand
                                }
                            };

                        layers.push(realand_ipc::LayerSurface {
                            namespace: surface.namespace().to_owned(),
                            output: name.clone(),
                            layer,
                            keyboard_interactivity,
                        });
                    }
                }

                let _ = tx.send_blocking(layers);
            });
            let result = rx.recv().await;
            let layers = result.map_err(|_| String::from("error getting layers info"))?;
            Response::Layers(layers)
        }
        Request::KeyboardLayouts => {
            let state = ctx.event_stream_state.borrow();
            let layout = state.keyboard_layouts.keyboard_layouts.clone();
            let layout = layout.expect("keyboard layouts should be set at startup");
            Response::KeyboardLayouts(layout)
        }
        Request::FocusedWindow => {
            let state = ctx.event_stream_state.borrow();
            let windows = &state.windows.windows;
            let window = windows.values().find(|win| win.is_focused).cloned();
            Response::FocusedWindow(window)
        }
        Request::PickWindow => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let pointer = state.realand.seat.get_pointer().unwrap();
                let start_data = PointerGrabStartData {
                    focus: None,
                    button: 0,
                    location: pointer.current_location(),
                };
                let grab = PickWindowGrab::new(start_data);
                // The `WindowPickGrab` ungrab handler will cancel the previous ongoing pick, if
                // any.
                pointer.set_grab(state, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
                state.realand.pick_window = Some(tx);
                state
                    .realand
                    .cursor_manager
                    .set_cursor_image(CursorImageStatus::Named(CursorIcon::Crosshair));
                // Redraw to update the cursor.
                state.realand.queue_redraw_all();
            });
            let result = rx.recv().await;
            let id = result.map_err(|_| String::from("error getting picked window info"))?;
            let window = id.and_then(|id| {
                let state = ctx.event_stream_state.borrow();
                state.windows.windows.get(&id.get()).cloned()
            });
            Response::PickedWindow(window)
        }
        Request::PickColor => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                state.handle_pick_color(tx);
            });
            let result = rx.recv().await;
            let color = result.map_err(|_| String::from("error getting picked color"))?;
            Response::PickedColor(color)
        }
        Request::Action(action) => {
            let restore_token = if matches!(&action, Action::RestoreSessionLayout { .. }) {
                let (tx, rx) = async_channel::bounded(1);
                ctx.event_loop.insert_idle(move |state| {
                    let _ = tx.try_send(state.begin_session_layout_restore());
                });
                Some(rx.recv().await.map_err(|e| e.to_string())??)
            } else {
                None
            };
            // Keep ordinary input actions immediate. Only owned file/session preparation
            // crosses the worker boundary; awaiting it preserves this connection's ordering.
            let (action, session) = if matches!(
                &action,
                Action::RestoreSessionLayout { .. } | Action::LoadConfigFile { .. }
            ) {
                ctx.owned_tasks
                    .submit(move || {
                        validate_action(&action)?;
                        let session = if let Action::RestoreSessionLayout { json } = &action {
                            Some(parse_session_layout(json)?)
                        } else {
                            None
                        };
                        Ok((realand_config::Action::from(action), session))
                    })?
                    .finish()
                    .await?
            } else {
                validate_action(&action)?;
                (realand_config::Action::from(action), None)
            };

            let (tx, rx) = async_channel::bounded(1);

            ctx.event_loop.insert_idle(move |state| {
                // Make sure some logic like workspace clean-up has a chance to run before doing
                // actions.
                state.realand.advance_animations();
                let result = if let Some(session) = session {
                    state.apply_session_layout_restore(
                        restore_token.expect("prepared session token"),
                        session,
                    )
                } else {
                    state.do_action(action, false);
                    Ok(())
                };
                let _ = tx.send_blocking(result);
            });

            // Wait until the action has been processed before returning. This is important for a
            // few actions, for instance for DoScreenTransition this wait ensures that the screen
            // contents were sampled into the texture.
            rx.recv().await.map_err(|e| e.to_string())??;
            Response::Handled
        }
        Request::Output { output, action } => {
            action.validate()?;

            let ipc_outputs = ctx.ipc_outputs.lock().unwrap();
            let found = ipc_outputs
                .values()
                .any(|o| OutputName::from_ipc_output(o).matches(&output));
            let response = if found {
                OutputConfigChanged::Applied
            } else {
                OutputConfigChanged::OutputWasMissing
            };
            drop(ipc_outputs);

            ctx.event_loop.insert_idle(move |state| {
                state.apply_transient_output_config(&output, action);
            });

            Response::OutputConfigChanged(response)
        }
        Request::PreviewOutputConfig {
            root_path,
            edited_path,
            content,
        } => {
            let outputs = ctx
                .owned_tasks
                .submit(move || {
                    crate::output_preview::parse_outputs(&root_path, &edited_path, content)
                        .map_err(|e| e.to_string())
                })?
                .finish()
                .await?;
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let result = state
                    .begin_output_preview(outputs, std::time::Duration::from_secs(15))
                    .map_err(|e| e.to_string());
                let _ = tx.send_blocking(result);
            });
            let id = rx.recv().await.map_err(|e| e.to_string())??;
            Response::OutputPreview {
                id,
                timeout_ms: 15_000,
            }
        }
        Request::ConfirmOutputPreview { id } | Request::CancelOutputPreview { id } => {
            let keep = matches!(request, Request::ConfirmOutputPreview { .. });
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let result = state
                    .finish_output_preview(id, keep)
                    .map_err(|e| e.to_string());
                let _ = tx.send_blocking(result);
            });
            rx.recv().await.map_err(|e| e.to_string())??;
            Response::Handled
        }
        Request::FocusedOutput => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let active_output = state
                    .realand
                    .layout
                    .active_output()
                    .map(|output| output.name());

                let output = active_output.and_then(|active_output| {
                    state
                        .backend
                        .ipc_outputs()
                        .lock()
                        .unwrap()
                        .values()
                        .find(|o| o.name == active_output)
                        .cloned()
                });

                let _ = tx.send_blocking(output);
            });
            let result = rx.recv().await;
            let output = result.map_err(|_| String::from("error getting active output info"))?;
            Response::FocusedOutput(output)
        }
        Request::EventStream => Response::Handled,
        Request::OverviewState => {
            let state = ctx.event_stream_state.borrow();
            let is_open = state.overview.is_open;
            Response::OverviewState(Overview { is_open })
        }
        Request::Casts => {
            let state = ctx.event_stream_state.borrow();
            let casts = state.casts.casts.values().cloned().collect();
            Response::Casts(casts)
        }
    };

    Ok(response)
}

pub(crate) fn parse_session_layout(
    json: &str,
) -> Result<realand_ipc::management::SessionLayout, String> {
    if json.len() > 4 * 1024 * 1024 {
        return Err("会话文件过大".into());
    }
    let session: realand_ipc::management::SessionLayout =
        serde_json::from_str(json).map_err(|e| e.to_string())?;
    session.validate()?;
    Ok(session)
}

fn validate_action(action: &Action) -> Result<(), String> {
    match action {
        Action::RestoreSessionLayout { json } => {
            if json.len() > 4 * 1024 * 1024 {
                return Err("会话文件过大".into());
            }
        }
        Action::SetWindowGroup { name, windows }
            if name.trim().is_empty()
                || name.len() > 128
                || windows.is_empty()
                || windows.len() > 4096 =>
        {
            return Err("无效的分组名称或成员".into())
        }
        _ => (),
    }

    if let Action::Screenshot { path, .. }
    | Action::ScreenshotScreen { path, .. }
    | Action::ScreenshotWindow { path, .. }
    | Action::LoadConfigFile { path } = action
    {
        if let Some(path) = path {
            // Relative paths are resolved against the realand compositor's working directory, which
            // is almost certainly not what you want.
            if !Path::new(path).is_absolute() {
                return Err(format!("path must be absolute: {path}"));
            }
        }
    }

    if let Action::LoadConfigFile { path: Some(path) } = action {
        let p = Path::new(path);
        if !p.is_file() {
            return Err(format!("path does not point to a file: {path}"));
        }
    }

    Ok(())
}

async fn handle_event_stream_client(client: EventStreamClient) -> anyhow::Result<()> {
    let EventStreamClient {
        events,
        disconnect,
        mut write,
    } = client;

    while let Ok(event) = events.recv().await {
        let buf = serialize_ipc_line(&event, "event")?;

        let res = select_biased! {
            _ = disconnect.recv().fuse() => return Ok(()),
            res = write.write_all(&buf).fuse() => res,
        };

        match res {
            Ok(()) => (),
            // Normal client disconnection.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            res @ Err(_) => res.context("error writing event")?,
        }
    }

    Ok(())
}

fn make_ipc_window(
    mapped: &Mapped,
    workspace_id: Option<WorkspaceId>,
    layout: WindowLayout,
) -> realand_ipc::Window {
    // Reading SurfaceData inside with_toplevel_role would acquire the same
    // Smithay surface mutex twice and deadlock IPC/focus refresh.
    let gtk_app_menu =
        crate::handlers::gtk_shell::surface_menu(mapped.toplevel().wl_surface()).map(Box::new);
    let app_menu =
        crate::handlers::qt_app_menu::surface_menu(mapped.toplevel().wl_surface()).map(Box::new);
    with_toplevel_role(mapped.toplevel(), |role| realand_ipc::Window {
        id: mapped.id().get(),
        title: role.title.clone(),
        app_id: role.app_id.clone(),
        pid: mapped.credentials().map(|c| c.pid),
        gtk_app_menu,
        app_menu,
        workspace_id: workspace_id.map(|id| id.get()),
        is_focused: mapped.is_focused(),
        is_minimized: mapped.is_minimized(),
        is_floating: mapped.is_floating(),
        is_urgent: mapped.is_urgent(),
        layout,
        focus_timestamp: mapped.get_focus_timestamp().map(Timestamp::from),
    })
}

impl State {
    pub fn ipc_keyboard_layouts_changed(&mut self) {
        let keyboard = self.realand.seat.get_keyboard().unwrap();
        let keyboard_layouts = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap();
            let layouts = xkb.layouts();
            KeyboardLayouts {
                names: layouts
                    .map(|layout| xkb.layout_name(layout).to_owned())
                    .collect(),
                current_idx: xkb.active_layout().0 as u8,
            }
        });

        let Some(server) = &self.realand.ipc_server else {
            return;
        };

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.keyboard_layouts;

        let event = Event::KeyboardLayoutsChanged { keyboard_layouts };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_refresh_keyboard_layout_index(&mut self) {
        let keyboard = self.realand.seat.get_keyboard().unwrap();
        let idx = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap();
            xkb.active_layout().0 as u8
        });

        let Some(server) = &self.realand.ipc_server else {
            return;
        };

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.keyboard_layouts;

        if state.keyboard_layouts.as_ref().unwrap().current_idx == idx {
            return;
        }

        let event = Event::KeyboardLayoutSwitched { idx };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_refresh_layout(&mut self) {
        self.ipc_refresh_workspaces();
        self.ipc_refresh_windows();
        self.ipc_refresh_overview();
    }

    fn ipc_refresh_workspaces(&mut self) {
        let Some(server) = &self.realand.ipc_server else {
            return;
        };

        let _span = realand_tracy_span!("State::ipc_refresh_workspaces");

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.workspaces;

        let mut events = Vec::new();
        let layout = &self.realand.layout;
        let focused_ws_id = layout.active_workspace().map(|ws| ws.id().get());

        // Check for workspace changes.
        let mut seen = HashSet::new();
        let mut need_workspaces_changed = false;
        for (mon, ws_idx, ws) in layout.workspaces() {
            let id = ws.id().get();
            seen.insert(id);

            let Some(ipc_ws) = state.workspaces.get(&id) else {
                // A new workspace was added.
                need_workspaces_changed = true;
                break;
            };

            // Check for any changes that we can't signal as individual events.
            let output_name = mon.map(|mon| mon.output_name());
            if ipc_ws.idx != u8::try_from(ws_idx + 1).unwrap_or(u8::MAX)
                || ipc_ws.name.as_deref() != ws.name().as_deref().map(String::as_str)
                || ipc_ws.output.as_ref() != output_name
            {
                need_workspaces_changed = true;
                break;
            }

            let active_window_id = ws.active_window().map(|win| win.id().get());
            if ipc_ws.active_window_id != active_window_id {
                events.push(Event::WorkspaceActiveWindowChanged {
                    workspace_id: id,
                    active_window_id,
                });
            }

            // Check if this workspace urgent state changed.
            let urgent = ws.is_urgent();
            if urgent != ipc_ws.is_urgent {
                events.push(Event::WorkspaceUrgencyChanged { id, urgent });
            }

            // Check if this workspace became focused.
            let is_focused = Some(id) == focused_ws_id;
            if is_focused && !ipc_ws.is_focused {
                events.push(Event::WorkspaceActivated { id, focused: true });
                continue;
            }

            // Check if this workspace became active.
            let is_active = mon.is_some_and(|mon| mon.active_workspace_idx() == ws_idx);
            if is_active && !ipc_ws.is_active {
                events.push(Event::WorkspaceActivated { id, focused: false });
            }
        }

        // Check if any workspaces were removed.
        if !need_workspaces_changed && state.workspaces.keys().any(|id| !seen.contains(id)) {
            need_workspaces_changed = true;
        }

        if need_workspaces_changed {
            events.clear();

            let workspaces = layout
                .workspaces()
                .map(|(mon, ws_idx, ws)| {
                    let id = ws.id().get();
                    Workspace {
                        id,
                        idx: u8::try_from(ws_idx + 1).unwrap_or(u8::MAX),
                        name: ws.name().map(|name| (*name).clone()),
                        output: mon.map(|mon| mon.output_name().clone()),
                        is_urgent: ws.is_urgent(),
                        is_active: mon.is_some_and(|mon| mon.active_workspace_idx() == ws_idx),
                        is_focused: Some(id) == focused_ws_id,
                        active_window_id: ws.active_window().map(|win| win.id().get()),
                    }
                })
                .collect();

            events.push(Event::WorkspacesChanged { workspaces });
        }

        for event in events {
            state.apply(event.clone());
            server.send_event(event);
        }
    }

    pub(crate) fn ipc_refresh_windows(&mut self) {
        let Some(server) = &self.realand.ipc_server else {
            return;
        };

        let _span = realand_tracy_span!("State::ipc_refresh_windows");

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.windows;

        let mut events = Vec::new();
        let layout = &self.realand.layout;

        let mut batch_change_layouts: Vec<(u64, WindowLayout)> = Vec::new();

        // Check for window changes.
        let mut seen = HashSet::new();
        let mut focused_id = None;
        layout.with_windows(|mapped, _, ws_id, window_layout| {
            let id = mapped.id().get();
            seen.insert(id);

            if mapped.is_focused() {
                focused_id = Some(id);
            }

            let Some(ipc_win) = state.windows.get(&id) else {
                let window = make_ipc_window(mapped, ws_id, window_layout);
                events.push(Event::WindowOpenedOrChanged { window });
                return;
            };

            let workspace_id = ws_id.map(|id| id.get());
            let mut changed = ipc_win.workspace_id != workspace_id
                || ipc_win.is_floating != mapped.is_floating()
                || ipc_win.is_minimized != mapped.is_minimized();

            changed |= with_toplevel_role(mapped.toplevel(), |role| {
                ipc_win.title != role.title || ipc_win.app_id != role.app_id
            });
            changed |= !crate::handlers::gtk_shell::surface_menu_matches(
                mapped.toplevel().wl_surface(),
                ipc_win.gtk_app_menu.as_deref(),
            );
            changed |= !crate::handlers::qt_app_menu::surface_menu_matches(
                mapped.toplevel().wl_surface(),
                ipc_win.app_menu.as_deref(),
            );

            if changed {
                let window = make_ipc_window(mapped, ws_id, window_layout);
                events.push(Event::WindowOpenedOrChanged { window });
                return;
            }

            if ipc_win.layout != window_layout {
                batch_change_layouts.push((id, window_layout));
            }

            if mapped.is_focused() && !ipc_win.is_focused {
                events.push(Event::WindowFocusChanged { id: Some(id) });
            }

            let focus_timestamp = mapped.get_focus_timestamp().map(Timestamp::from);
            if focus_timestamp != ipc_win.focus_timestamp {
                events.push(Event::WindowFocusTimestampChanged {
                    id,
                    focus_timestamp,
                });
            }

            let urgent = mapped.is_urgent();
            if urgent != ipc_win.is_urgent {
                events.push(Event::WindowUrgencyChanged { id, urgent })
            }
        });

        // It might make sense to push layout changes after closed windows (since windows about to
        // be closed will occupy the same column/tile positions as the window that moved into this
        // vacated space), but also we are already pushing some layout changes in
        // WindowOpenedOrChanged above, meaning that the receiving end has to handle this case
        // anyway.
        if !batch_change_layouts.is_empty() {
            events.push(Event::WindowLayoutsChanged {
                changes: batch_change_layouts,
            });
        }

        // Check for closed windows.
        let mut ipc_focused_id = None;
        for (id, ipc_win) in &state.windows {
            if !seen.contains(id) {
                events.push(Event::WindowClosed { id: *id });
            }

            if ipc_win.is_focused {
                ipc_focused_id = Some(id);
            }
        }

        // Extra check for focus becoming None, since the checks above only work for focus becoming
        // a different window.
        if focused_id.is_none() && ipc_focused_id.is_some() {
            events.push(Event::WindowFocusChanged { id: None });
        }

        for event in events {
            state.apply(event.clone());
            server.send_event(event);
        }
    }

    pub fn ipc_refresh_overview(&mut self) {
        let Some(server) = &self.realand.ipc_server else {
            return;
        };

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.overview;
        let is_open = self.realand.layout.is_overview_open();

        if state.is_open == is_open {
            return;
        }

        let event = Event::OverviewOpenedOrClosed { is_open };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_refresh_casts(&mut self) {
        let Some(server) = &self.realand.ipc_server else {
            return;
        };

        let _span = realand_tracy_span!("State::ipc_refresh_casts");

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.casts;

        let mut events = Vec::new();
        let mut seen = HashSet::new();

        // Check PipeWire screencasts.
        #[cfg(feature = "xdp-gnome-screencast")]
        {
            // Check pending dynamic casts.
            for pending in &self.realand.casting.pending_dynamic_casts {
                let stream_id = pending.stream_id.get();
                seen.insert(stream_id);

                // Pending dynamic casts don't change any properties, so we only need to check if
                // it's missing from the state.
                if !state.casts.contains_key(&stream_id) {
                    let cast = realand_ipc::Cast {
                        session_id: pending.session_id.get(),
                        stream_id,
                        kind: realand_ipc::CastKind::PipeWire,
                        target: realand_ipc::CastTarget::Nothing {},
                        is_dynamic_target: true,
                        is_active: false,
                        pid: None,
                        pw_node_id: None,
                    };
                    events.push(Event::CastStartedOrChanged { cast });
                }
            }

            // Check active casts.
            for cast in &self.realand.casting.casts {
                let stream_id = cast.stream_id.get();
                seen.insert(stream_id);

                let pw_node_id = cast.node_id();
                if state.casts.get(&stream_id).is_none_or(|existing| {
                    // Only these properties can change.
                    existing.is_active != cast.is_active()
                        || !cast.target.matches(&existing.target)
                        || existing.pw_node_id != pw_node_id
                }) {
                    let cast = realand_ipc::Cast {
                        session_id: cast.session_id.get(),
                        stream_id,
                        kind: realand_ipc::CastKind::PipeWire,
                        target: cast.target.make_ipc(),
                        is_dynamic_target: cast.dynamic_target,
                        is_active: cast.is_active(),
                        pid: None,
                        pw_node_id,
                    };
                    events.push(Event::CastStartedOrChanged { cast });
                }
            }
        }

        // Check screencopy casts.
        //
        // First, clear expired casts. Ideally we'd have a deadline timer, but our 1 second frame
        // callback timer calls refresh regularly, so that's fine as is.
        self.realand.screencopy_state.clear_expired_casts();

        for queue in self.realand.screencopy_state.queues() {
            if let Some(cast_info) = queue.cast() {
                let stream_id = cast_info.stream_id.get();
                seen.insert(stream_id);

                if state.casts.get(&stream_id).is_none_or(|existing| {
                    // Only this property can change.
                    match &existing.target {
                        realand_ipc::CastTarget::Output { name } => *name != cast_info.output_name,
                        _ => true,
                    }
                }) {
                    let cast = realand_ipc::Cast {
                        session_id: cast_info.session_id.get(),
                        stream_id,
                        kind: realand_ipc::CastKind::WlrScreencopy,
                        target: realand_ipc::CastTarget::Output {
                            name: cast_info.output_name.clone(),
                        },
                        is_dynamic_target: false,
                        is_active: true,
                        pid: queue.credentials().map(|creds| creds.pid),
                        pw_node_id: None,
                    };
                    events.push(Event::CastStartedOrChanged { cast });
                }
            }
        }

        // Check for stopped casts.
        for stream_id in state.casts.keys() {
            if !seen.contains(stream_id) {
                events.push(Event::CastStopped {
                    stream_id: *stream_id,
                });
            }
        }

        for event in events {
            state.apply(event.clone());
            server.send_event(event);
        }
    }

    pub fn ipc_config_loaded(&mut self, failed: bool) {
        let Some(server) = &self.realand.ipc_server else {
            return;
        };
        let mut state = server.event_stream_state.borrow_mut();

        let event = Event::ConfigLoaded { failed };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_screenshot_taken(&mut self, path: Option<String>) {
        let Some(server) = &self.realand.ipc_server else {
            return;
        };
        let mut state = server.event_stream_state.borrow_mut();

        let event = Event::ScreenshotCaptured { path };
        state.apply(event.clone());
        server.send_event(event);
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use futures_util::io::Cursor;

    #[test]
    fn prepared_session_is_bounded_and_semantically_validated() {
        let valid = r#"{"version":1,"workspaces":[],"windows":[],"groups":[]}"#;
        assert!(parse_session_layout(valid).is_ok());
        assert!(parse_session_layout(&valid.replace("\"version\":1", "\"version\":2")).is_err());
        assert!(parse_session_layout("{}").is_err());
        assert_eq!(
            parse_session_layout(&"x".repeat(4 * 1024 * 1024 + 1)).unwrap_err(),
            "会话文件过大"
        );
    }

    #[test]
    fn serialized_ipc_lines_include_the_newline_in_the_limit() {
        let exact = "x".repeat(MAX_IPC_LINE_BYTES - 3);
        assert_eq!(
            serialize_ipc_line(&exact, "test").unwrap().len(),
            MAX_IPC_LINE_BYTES
        );

        let oversized = "x".repeat(MAX_IPC_LINE_BYTES - 2);
        let error = serialize_ipc_line(&oversized, "test").unwrap_err();
        assert!(error.to_string().contains("IPC test too large"));
    }

    #[test]
    fn oversized_unterminated_request_is_bounded() {
        let mut reader = Cursor::new(vec![b'x'; 1024]);
        let error = read_request(&mut reader, 32)
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(reader.position(), 33);
    }

    #[test]
    fn pipelined_requests_are_not_consumed_together() {
        let mut reader = Cursor::new(b"\"Version\"\n\"Windows\"\n");
        assert_eq!(
            read_request(&mut reader, 32)
                .now_or_never()
                .unwrap()
                .unwrap(),
            b"\"Version\"\n"
        );
        assert_eq!(
            read_request(&mut reader, 32)
                .now_or_never()
                .unwrap()
                .unwrap(),
            b"\"Windows\"\n"
        );
        assert!(read_request(&mut reader, 32)
            .now_or_never()
            .unwrap()
            .unwrap()
            .is_empty());
    }
}

#[cfg(test)]
mod socket_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static NEXT_SOCKET_ID: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn ipc_socket_is_private() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let suffix = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "realand-ipc-permissions-{}-{nonce}-{suffix}.sock",
            process::id()
        ));

        let listener = bind_private_socket(&path).expect("bind private IPC socket");
        let mode = fs::metadata(&path)
            .expect("stat private IPC socket")
            .permissions()
            .mode()
            & 0o777;
        drop(listener);
        let _ = fs::remove_file(&path);

        assert_eq!(mode, 0o600);
    }

    #[test]
    fn ipc_accepts_the_compositor_user() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        check_ipc_peer(&stream).unwrap();
    }
}
