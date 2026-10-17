//! Deliver protocol destruction before dropping the application state.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use calloop::generic::Generic;
use smithay::reexports::wayland_server::backend::{ClientId, DisconnectReason};
use smithay::reexports::wayland_server::Display;

/// Only clients successfully admitted by this compositor, pruned on disconnect.
/// Keep IDs rather than Client/Display owners so the registry cannot retain any
/// protocol userdata or backend. Connection changes never run in the frame path.
#[derive(Default)]
pub(crate) struct ClientRegistry {
    clients: Arc<Mutex<Vec<ClientId>>>,
}

impl ClientRegistry {
    pub(crate) fn registration(&self) -> ClientRegistration {
        ClientRegistration {
            clients: Arc::downgrade(&self.clients),
            disconnected: AtomicBool::new(false),
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<ClientId> {
        self.clients.lock().unwrap().clone()
    }
}

pub(crate) struct ClientRegistration {
    clients: Weak<Mutex<Vec<ClientId>>>,
    disconnected: AtomicBool,
}

impl ClientRegistration {
    /// Call only after DisplayHandle::insert_client succeeds. The disconnected
    /// flag also covers a concurrent disconnect before insertion returns.
    pub(crate) fn connected(&self, client: ClientId) {
        if let Some(clients) = self.clients.upgrade() {
            let mut clients = clients.lock().unwrap();
            if !self.disconnected.load(Ordering::Acquire) {
                clients.push(client);
            }
        }
    }

    pub(crate) fn disconnected(&self, client: ClientId) {
        self.disconnected.store(true, Ordering::Release);
        if let Some(clients) = self.clients.upgrade() {
            clients.lock().unwrap().retain(|id| id != &client);
        }
    }
}

/// Kill every existing client before dispatching. No listener or calloop event
/// source is dispatched here, and no live connection remains to send requests.
/// Backend destruction callbacks need `data` to break protocol-owned references.
pub(crate) fn disconnect_clients<Data: 'static>(
    source: &mut Generic<Display<Data>>,
    data: &mut Data,
    clients: Vec<ClientId>,
) -> io::Result<usize> {
    // SAFETY: dispatch does not move the Display or change its registered fd.
    let display = unsafe { source.get_mut() };
    let backend = display.handle().backend_handle();
    // Never enumerate through with_all_clients: the pinned sys backend treats
    // libwayland's list sentinel as a client and loops for a non-empty display.
    // The snapshot was collected outside backend calls; kill_client may invoke
    // ClientData::disconnected synchronously and re-enter the registry.
    let count = clients.len();
    for client in clients {
        backend.kill_client(client, DisconnectReason::ConnectionClosed);
    }
    // libwayland queues Rust destruction callbacks during kill_client. Merely
    // dropping Display would discard them without Smithay's surface cleanup.
    display.dispatch_clients(data)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use calloop::{Interest, Mode};
    use smithay::reexports::wayland_server::backend::ClientData;

    use super::*;

    struct RegisteredClient(ClientRegistration);

    impl ClientData for RegisteredClient {
        fn disconnected(&self, client: ClientId, _: DisconnectReason) {
            self.0.disconnected(client);
        }
    }

    #[test]
    fn live_client_snapshot_prunes_disconnects_and_kills_every_remaining_client() {
        let display = Display::<()>::new().unwrap();
        let mut handle = display.handle();
        let registry = ClientRegistry::default();
        let mut peers = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..3 {
            let (socket, peer) = UnixStream::pair().unwrap();
            let data = Arc::new(RegisteredClient(registry.registration()));
            let client = handle.insert_client(socket, data.clone()).unwrap();
            data.0.connected(client.id());
            ids.push(client.id());
            peers.push(peer);
        }
        assert_eq!(registry.snapshot().len(), 3);
        handle
            .backend_handle()
            .kill_client(ids[0].clone(), DisconnectReason::ConnectionClosed);
        assert_eq!(registry.snapshot().len(), 2);
        let clients = registry.snapshot();
        let mut source = Generic::new(display, Interest::READ, Mode::Level);
        assert_eq!(
            disconnect_clients(&mut source, &mut (), clients).unwrap(),
            2
        );
        assert!(registry.snapshot().is_empty());
        drop(peers);
    }

    #[test]
    fn disconnect_before_success_registration_cannot_resurrect_client_id() {
        let display = Display::<()>::new().unwrap();
        let mut handle = display.handle();
        let registry = ClientRegistry::default();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let data = Arc::new(RegisteredClient(registry.registration()));
        let client = handle.insert_client(socket, data.clone()).unwrap();
        handle
            .backend_handle()
            .kill_client(client.id(), DisconnectReason::ConnectionClosed);
        data.0.connected(client.id());
        assert!(registry.snapshot().is_empty());
    }
}
