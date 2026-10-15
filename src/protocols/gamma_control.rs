use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use calloop::channel::{Channel, Event};
use calloop::LoopHandle;

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::{Dispatch2, GlobalDispatch2};
use wayland_protocols_wlr::gamma_control::v1::server::{
    zwlr_gamma_control_manager_v1, zwlr_gamma_control_v1,
};
use zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;
use zwlr_gamma_control_v1::ZwlrGammaControlV1;

use crate::protocols::EmptyData;

const VERSION: u32 = 1;
#[path = "gamma_control/reader.rs"]
mod reader;

struct Pending {
    id: u64,
    cancel: Arc<AtomicBool>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
struct ActiveControl {
    resource: ZwlrGammaControlV1,
    pending: Option<Pending>,
}

pub struct GammaControlManagerState {
    // Active gamma controls only. Failed ones are removed.
    gamma_controls: HashMap<Output, ActiveControl>,
    reader: reader::Reader,
    completions: Option<Channel<reader::Completion>>,
    next_request: u64,
}

pub struct GammaControlManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

pub trait GammaControlHandler {
    fn gamma_control_manager_state(&mut self) -> &mut GammaControlManagerState;
    fn get_gamma_size(&mut self, output: &Output) -> Option<u32>;
    fn set_gamma(&mut self, output: &Output, ramp: Option<Vec<u16>>) -> Option<()>;
}

pub struct GammaControlState {
    gamma_size: u32,
}

impl GammaControlManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwlrGammaControlManagerV1, GammaControlManagerGlobalData>,
        D: GammaControlHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = GammaControlManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, ZwlrGammaControlManagerV1, _>(VERSION, global_data);

        let (reader, completions) = reader::Reader::new();
        Self {
            gamma_controls: HashMap::new(),
            reader,
            completions: Some(completions),
            next_request: 0,
        }
    }

    /// Register once on the compositor owner. Pure LUT results are delivered only when the
    /// resource/output/request identity is still current; worker code never receives D.
    pub(crate) fn install_readback_source<D>(
        &mut self,
        event_loop: &LoopHandle<'static, D>,
    ) -> anyhow::Result<()>
    where
        D: GammaControlHandler + 'static,
    {
        let Some(source) = self.completions.take() else {
            return Ok(());
        };
        event_loop
            .insert_source(source, |event, _, state| {
                if let Event::Msg(completion) = event {
                    Self::complete(state, completion);
                }
            })
            .map_err(|error| anyhow::anyhow!("cannot register gamma reader: {}", error.error))?;
        Ok(())
    }

    fn complete<D: GammaControlHandler>(state: &mut D, completion: reader::Completion) {
        let manager = state.gamma_control_manager_state();
        let target = manager
            .gamma_controls
            .iter_mut()
            .find_map(|(output, control)| {
                if !control.resource.is_alive()
                    || !control
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.id == completion.id)
                {
                    return None;
                }
                control.pending.take();
                Some((output.clone(), control.resource.clone()))
            });
        let Some((output, resource)) = target else {
            return;
        };
        let result = match completion.result {
            Ok(gamma) => state.set_gamma(&output, Some(gamma)),
            Err(error) => {
                warn!("failed to read gamma payload: {error}");
                None
            }
        };
        if result.is_none() {
            let controls = &mut state.gamma_control_manager_state().gamma_controls;
            if controls
                .get(&output)
                .is_some_and(|control| control.resource == resource)
            {
                resource.failed();
                controls.remove(&output);
                let _ = state.set_gamma(&output, None);
            }
        }
    }

    pub fn output_removed(&mut self, output: &Output) {
        if let Some(gamma_control) = self.gamma_controls.remove(output) {
            gamma_control.resource.failed();
        }
    }
}

impl<D> GlobalDispatch2<ZwlrGammaControlManagerV1, D> for GammaControlManagerGlobalData
where
    D: Dispatch<ZwlrGammaControlManagerV1, EmptyData>,
    D: GammaControlHandler,
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        manager: New<ZwlrGammaControlManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, EmptyData);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<ZwlrGammaControlManagerV1, D> for EmptyData
where
    D: Dispatch<ZwlrGammaControlV1, GammaControlState>,
    D: GammaControlHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _resource: &ZwlrGammaControlManagerV1,
        request: <ZwlrGammaControlManagerV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwlr_gamma_control_manager_v1::Request::GetGammaControl { id, output } => {
                if let Some(output) = Output::from_resource(&output) {
                    // We borrow state in the middle.
                    #[allow(clippy::map_entry)]
                    if !state
                        .gamma_control_manager_state()
                        .gamma_controls
                        .contains_key(&output)
                    {
                        if let Some(gamma_size) = state.get_gamma_size(&output) {
                            let zwlr_gamma_control =
                                data_init.init(id, GammaControlState { gamma_size });
                            zwlr_gamma_control.gamma_size(gamma_size);
                            state.gamma_control_manager_state().gamma_controls.insert(
                                output,
                                ActiveControl {
                                    resource: zwlr_gamma_control,
                                    pending: None,
                                },
                            );
                            return;
                        }
                    }
                }

                data_init
                    .init(id, GammaControlState { gamma_size: 0 })
                    .failed();
            }
            zwlr_gamma_control_manager_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch2<ZwlrGammaControlV1, D> for GammaControlState
where
    D: GammaControlHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        resource: &ZwlrGammaControlV1,
        request: <ZwlrGammaControlV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwlr_gamma_control_v1::Request::SetGamma { fd } => {
                let manager = state.gamma_control_manager_state();
                let Some(output) = manager.gamma_controls.iter().find_map(|(output, control)| {
                    (&control.resource == resource).then(|| output.clone())
                }) else {
                    return;
                };
                trace!("setting gamma for output {}", output.name());
                manager.next_request = manager
                    .next_request
                    .checked_add(1)
                    .expect("gamma request ID exhausted");
                let request = manager.next_request;
                let control = manager.gamma_controls.get_mut(&output).unwrap();
                // Retiring this token invalidates the old LUT even if its read already finished.
                control.pending.take();
                let cancel = Arc::new(AtomicBool::new(false));
                let result = manager
                    .reader
                    .request(request, fd, self.gamma_size, cancel.clone());
                if let Err(error) = result {
                    warn!("gamma readback cannot be queued: {error}");
                    resource.failed();
                    manager.gamma_controls.remove(&output);
                    let _ = state.set_gamma(&output, None);
                } else {
                    manager.gamma_controls.get_mut(&output).unwrap().pending = Some(Pending {
                        id: request,
                        cancel,
                    });
                }
            }
            zwlr_gamma_control_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ZwlrGammaControlV1) {
        let gamma_controls = &mut state.gamma_control_manager_state().gamma_controls;
        let Some((output, _)) = gamma_controls
            .iter()
            .find(|(_, control)| &control.resource == resource)
        else {
            return;
        };
        let output = output.clone();
        gamma_controls.remove(&output);

        let _ = state.set_gamma(&output, None);
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use smithay::output::{PhysicalProperties, Subpixel};
    use smithay::reexports::wayland_server::{backend::ClientData, Display};
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    struct TestState {
        manager: GammaControlManagerState,
        applied: Vec<(Output, Option<Vec<u16>>)>,
        heartbeats: usize,
    }
    impl GammaControlHandler for TestState {
        fn gamma_control_manager_state(&mut self) -> &mut GammaControlManagerState {
            &mut self.manager
        }
        fn get_gamma_size(&mut self, _: &Output) -> Option<u32> {
            Some(1)
        }
        fn set_gamma(&mut self, output: &Output, ramp: Option<Vec<u16>>) -> Option<()> {
            self.applied.push((output.clone(), ramp));
            Some(())
        }
    }
    impl Dispatch<ZwlrGammaControlV1, GammaControlState> for TestState {
        fn request(
            state: &mut Self,
            client: &Client,
            resource: &ZwlrGammaControlV1,
            request: zwlr_gamma_control_v1::Request,
            data: &GammaControlState,
            handle: &DisplayHandle,
            data_init: &mut DataInit<'_, Self>,
        ) {
            <GammaControlState as Dispatch2<ZwlrGammaControlV1, Self>>::request(
                data, state, client, resource, request, handle, data_init,
            );
        }
        fn destroyed(
            state: &mut Self,
            client: ClientId,
            resource: &ZwlrGammaControlV1,
            data: &GammaControlState,
        ) {
            <GammaControlState as Dispatch2<ZwlrGammaControlV1, Self>>::destroyed(
                data, state, client, resource,
            );
        }
    }
    #[derive(Debug)]
    struct TestClient;
    impl ClientData for TestClient {}
    fn output() -> Output {
        Output::new(
            "gamma-headless".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: String::new(),
            },
        )
    }
    fn fixture() -> (
        Display<TestState>,
        calloop::EventLoop<'static, TestState>,
        TestState,
        Client,
        UnixStream,
    ) {
        let display = Display::new().unwrap();
        let mut handle = display.handle();
        let (server, peer) = UnixStream::pair().unwrap();
        let client = handle.insert_client(server, Arc::new(TestClient)).unwrap();
        let (reader, completions) = reader::Reader::new();
        let mut state = TestState {
            manager: GammaControlManagerState {
                gamma_controls: HashMap::new(),
                reader,
                completions: Some(completions),
                next_request: 0,
            },
            applied: Vec::new(),
            heartbeats: 0,
        };
        let events = calloop::EventLoop::try_new().unwrap();
        state
            .manager
            .install_readback_source(&events.handle())
            .unwrap();
        (display, events, state, client, peer)
    }
    fn control(
        display: &Display<TestState>,
        state: &mut TestState,
        client: &Client,
        output: &Output,
    ) -> ZwlrGammaControlV1 {
        let resource = client
            .create_resource::<ZwlrGammaControlV1, _, TestState>(
                &display.handle(),
                1,
                GammaControlState { gamma_size: 1 },
            )
            .unwrap();
        state.manager.gamma_controls.insert(
            output.clone(),
            ActiveControl {
                resource: resource.clone(),
                pending: None,
            },
        );
        resource
    }
    fn pipe(payload: &[u8]) -> (OwnedFd, std::fs::File) {
        let mut pair = [0; 2];
        // SAFETY: two returned descriptors have independent owned wrappers below.
        assert_eq!(
            unsafe { libc::pipe2(pair.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let (fd, mut writer) = unsafe {
            (
                OwnedFd::from_raw_fd(pair[0]),
                std::fs::File::from_raw_fd(pair[1]),
            )
        };
        writer.write_all(payload).unwrap();
        (fd, writer)
    }
    fn send_gamma(peer: &UnixStream, resource: &ZwlrGammaControlV1, fd: &OwnedFd) {
        let message = [
            resource.id().protocol_id().to_ne_bytes(),
            (8u32 << 16).to_ne_bytes(),
        ]
        .concat();
        let mut iov = libc::iovec {
            iov_base: message.as_ptr().cast_mut().cast(),
            iov_len: message.len(),
        };
        let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) } as usize;
        let mut control = vec![0usize; space.div_ceil(std::mem::size_of::<usize>())];
        let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
        header.msg_iov = &mut iov;
        header.msg_iovlen = 1;
        header.msg_control = control.as_mut_ptr().cast();
        header.msg_controllen = space;
        // SAFETY: CMSG buffer is suitably aligned and sized for one borrowed FD; sendmsg copies it.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&header);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize;
            libc::CMSG_DATA(cmsg).cast::<i32>().write(fd.as_raw_fd());
            assert_eq!(
                libc::sendmsg(peer.as_raw_fd(), &header, libc::MSG_NOSIGNAL),
                8
            );
        }
    }
    fn dispatch_until(
        events: &mut calloop::EventLoop<'static, TestState>,
        state: &mut TestState,
        done: impl Fn(&TestState) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done(state) {
            assert!(Instant::now() < deadline, "gamma completion did not arrive");
            events
                .dispatch(Some(Duration::from_millis(10)), state)
                .unwrap();
            state.heartbeats += 1;
        }
    }
    #[test]
    fn actual_fd_request_leaves_main_dispatch_responsive_until_open_pipe_deadline() {
        let (mut display, mut events, mut state, client, peer) = fixture();
        let output = output();
        let resource = control(&display, &mut state, &client, &output);
        let (fd, _writer) = pipe(&[1, 0, 2, 0, 3, 0]);
        send_gamma(&peer, &resource, &fd);
        display.dispatch_clients(&mut state).unwrap();
        events.handle().insert_idle(|state| state.heartbeats += 1);
        events.dispatch(Some(Duration::ZERO), &mut state).unwrap();
        assert_eq!(state.heartbeats, 1);
        assert!(state.applied.is_empty());
        dispatch_until(&mut events, &mut state, |state| {
            !state.manager.gamma_controls.contains_key(&output)
        });
        assert!(state.heartbeats > 10);
        assert_eq!(state.applied.len(), 1);
        assert_eq!(state.applied[0].1, None);
    }
    #[test]
    fn out_of_order_old_lut_cannot_override_new_request() {
        let (mut display, mut events, mut state, client, peer) = fixture();
        let output = output();
        let resource = control(&display, &mut state, &client, &output);
        let (first, writer) = pipe(&[1, 0, 2, 0, 3, 0]);
        drop(writer);
        send_gamma(&peer, &resource, &first);
        display.dispatch_clients(&mut state).unwrap();
        let (second, writer) = pipe(&[4, 0, 5, 0, 6, 0]);
        drop(writer);
        send_gamma(&peer, &resource, &second);
        display.dispatch_clients(&mut state).unwrap();
        dispatch_until(&mut events, &mut state, |state| !state.applied.is_empty());
        for _ in 0..4 {
            events
                .dispatch(Some(Duration::from_millis(10)), &mut state)
                .unwrap();
        }
        assert_eq!(state.applied.len(), 1);
        assert_eq!(state.applied[0].1, Some(vec![4, 5, 6]));
    }
    #[test]
    fn removed_output_with_same_connector_replacement_discards_old_lut() {
        let (mut display, mut events, mut state, client, peer) = fixture();
        let old_output = output();
        let resource = control(&display, &mut state, &client, &old_output);
        let (first, writer) = pipe(&[1, 0, 2, 0, 3, 0]);
        drop(writer);
        send_gamma(&peer, &resource, &first);
        display.dispatch_clients(&mut state).unwrap();
        state.manager.output_removed(&old_output);
        let new_output = output();
        assert_ne!(old_output, new_output);
        let resource = control(&display, &mut state, &client, &new_output);
        let (second, writer) = pipe(&[4, 0, 5, 0, 6, 0]);
        drop(writer);
        send_gamma(&peer, &resource, &second);
        display.dispatch_clients(&mut state).unwrap();
        dispatch_until(&mut events, &mut state, |state| !state.applied.is_empty());
        for _ in 0..4 {
            events
                .dispatch(Some(Duration::from_millis(10)), &mut state)
                .unwrap();
        }
        assert_eq!(state.applied, [(new_output, Some(vec![4, 5, 6]))]);
    }
    #[test]
    fn destroyed_resource_cancels_pending_payload_and_resets_only_its_control() {
        let (mut display, mut events, mut state, client, mut peer) = fixture();
        let output = output();
        let resource = control(&display, &mut state, &client, &output);
        let (fd, _writer) = pipe(&[1, 0]);
        send_gamma(&peer, &resource, &fd);
        display.dispatch_clients(&mut state).unwrap();
        let destroy = [
            resource.id().protocol_id().to_ne_bytes(),
            ((8u32 << 16) | 1).to_ne_bytes(),
        ]
        .concat();
        peer.write_all(&destroy).unwrap();
        display.dispatch_clients(&mut state).unwrap();
        assert!(!state.manager.gamma_controls.contains_key(&output));
        for _ in 0..8 {
            events
                .dispatch(Some(Duration::from_millis(10)), &mut state)
                .unwrap();
        }
        assert_eq!(state.applied, [(output, None)]);
    }
}
