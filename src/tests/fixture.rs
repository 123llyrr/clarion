use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;
use std::time::Duration;

use calloop::generic::Generic;
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction};
use clarion_config::Config;
use smithay::output::Output;

use super::client::{Client, ClientId};
use super::server::Server;
use crate::compositor::{Clarion, NewClient};

pub struct Fixture {
    pub event_loop: EventLoop<'static, State>,
    pub handle: LoopHandle<'static, State>,
    pub state: State,
}

pub struct State {
    pub server: Server,
    pub clients: Vec<Client>,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_config(Config::default())
    }

    pub fn with_config(config: Config) -> Self {
        Self::with_server(Server::new(config))
    }

    pub fn with_config_and_renderer(
        config: Config,
        renderer: crate::backend::RendererBackendType,
    ) -> Self {
        Self::with_server(Server::with_renderer(config, renderer))
    }

    fn with_server(server: Server) -> Self {
        let event_loop = EventLoop::try_new().unwrap();
        let handle = event_loop.handle();

        let fd = server.event_loop.as_fd().try_clone_to_owned().unwrap();
        let source = Generic::new(fd, Interest::READ, Mode::Level);
        handle
            .insert_source(source, |_, _, state: &mut State| {
                state.server.dispatch();
                Ok(PostAction::Continue)
            })
            .unwrap();

        let state = State {
            server,
            clients: Vec::new(),
        };

        Self {
            event_loop,
            handle,
            state,
        }
    }

    pub fn dispatch(&mut self) {
        self.event_loop
            .dispatch(Duration::ZERO, &mut self.state)
            .unwrap();
    }

    pub fn clarion_state(&mut self) -> &mut crate::compositor::State {
        &mut self.state.server.state
    }

    pub fn clarion(&mut self) -> &mut Clarion {
        &mut self.clarion_state().clarion
    }

    pub fn clarion_output(&self, n: u8) -> Output {
        let clarion = &self.state.server.state.clarion;
        let idx = usize::from(n - 1);
        let output = clarion.global_space.outputs().nth(idx).unwrap();
        output.clone()
    }

    pub fn clarion_focus_output(&mut self, n: u8) {
        let clarion = &mut self.state.server.state.clarion;
        let idx = usize::from(n - 1);
        let output = clarion.global_space.outputs().nth(idx).unwrap();
        clarion.layout.focus_output(output);
    }

    pub fn clarion_complete_animations(&mut self) {
        let clarion = self.clarion();
        clarion.clock.set_complete_instantly(true);
        clarion.advance_animations();
        clarion.clock.set_complete_instantly(false);
    }

    /// Keep one display alive across concurrent GPU fixtures. Smithay's weak
    /// display registry otherwise races a final display drop against a new
    /// surfaceless display lookup during dense mode-transition matrices.
    pub fn init_gles_renderer(&mut self) {
        use std::sync::OnceLock;

        use smithay::backend::egl::native::EGLSurfacelessDisplay;
        use smithay::backend::egl::EGLDisplay;

        static DISPLAY: OnceLock<EGLDisplay> = OnceLock::new();
        DISPLAY.get_or_init(|| unsafe { EGLDisplay::new(EGLSurfacelessDisplay).unwrap() });
        self.clarion_state()
            .backend
            .headless()
            .add_renderer()
            .unwrap();
    }

    pub fn add_output(&mut self, n: u8, size: (u16, u16)) {
        let state = self.clarion_state();
        let clarion = &mut state.clarion;
        state.backend.headless().add_output(
            &mut crate::platform::device_bridge::DeviceCoordinator::new(clarion),
            n,
            size,
        );
    }

    pub fn add_client(&mut self) -> ClientId {
        let (sock1, sock2) = UnixStream::pair().unwrap();
        self.clarion().insert_client(NewClient {
            client: sock1,
            restricted: false,
            credentials_unknown: false,
        });

        let client = Client::new(sock2);
        let id = client.id;

        let fd = client.event_loop.as_fd().try_clone_to_owned().unwrap();
        let source = Generic::new(fd, Interest::READ, Mode::Level);
        self.handle
            .insert_source(source, move |_, _, state: &mut State| {
                state.client(id).dispatch();
                Ok(PostAction::Continue)
            })
            .unwrap();

        self.state.clients.push(client);
        self.roundtrip(id);
        id
    }

    pub fn client(&mut self, id: ClientId) -> &mut Client {
        self.state.client(id)
    }

    pub fn roundtrip(&mut self, id: ClientId) {
        let client = self.state.client(id);
        let data = client.send_sync();
        while !data.done.load(Ordering::Relaxed) {
            self.dispatch();
        }
    }

    /// Roundtrip twice in a row.
    ///
    /// For some reason, when running tests on many threads at once, a single roundtrip is
    /// sometimes not sufficient to get the configure events to the client.
    ///
    /// I suspect that this is because these configure events are sent from the clarion loop
    /// callback, so they arrive after the sync done event and don't get processed in that
    /// client dispatch cycle. I'm not sure why this would be dependent on multithreading. But
    /// if this is indeed the issue, then a double roundtrip fixes it.
    pub fn double_roundtrip(&mut self, id: ClientId) {
        self.roundtrip(id);
        self.roundtrip(id);
    }
}

impl State {
    pub fn client(&mut self, id: ClientId) -> &mut Client {
        self.clients.iter_mut().find(|c| c.id == id).unwrap()
    }
}
