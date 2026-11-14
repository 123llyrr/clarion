use futures_util::StreamExt;
use zbus::fdo;
use zbus::names::InterfaceName;
use zbus::zvariant::OwnedObjectPath;

pub enum Login1ToClarion {
    LidClosedChanged(bool),
    /// logind asked this session to lock (`loginctl lock-session`, or any other
    /// caller of `Manager.LockSession`).
    LockRequested,
}

pub fn start(
    to_clarion: calloop::channel::Sender<Login1ToClarion>,
) -> anyhow::Result<zbus::blocking::Connection> {
    let conn = zbus::blocking::Connection::system()?;

    let async_conn = conn.inner().clone();
    let props_conn = async_conn.clone();
    let future = {
        let to_clarion = to_clarion.clone();
        async move {
            let proxy = fdo::PropertiesProxy::new(
                &props_conn,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
            )
            .await;
            let proxy = match proxy {
                Ok(x) => x,
                Err(err) => {
                    warn!("error creating PropertiesProxy: {err:?}");
                    return;
                }
            };

            let mut props_changed = match proxy.receive_properties_changed().await {
                Ok(x) => x,
                Err(err) => {
                    warn!("error subscribing to PropertiesChanged: {err:?}");
                    return;
                }
            };

            let props = proxy
                .get_all(InterfaceName::try_from("org.freedesktop.login1.Manager").unwrap())
                .await;
            let mut props = match props {
                Ok(x) => x,
                Err(err) => {
                    warn!("error receiving initial properties: {err:?}");
                    return;
                }
            };

            trace!("initial properties: {props:?}");

            let mut lid_closed = props
                .remove("LidClosed")
                .and_then(|value| bool::try_from(value).ok())
                .unwrap_or_default();

            if let Err(err) = to_clarion.send(Login1ToClarion::LidClosedChanged(lid_closed)) {
                warn!("error sending initial lid state to clarion: {err:?}");
                return;
            };

            while let Some(signal) = props_changed.next().await {
                let args = match signal.args() {
                    Ok(args) => args,
                    Err(err) => {
                        warn!("error parsing PropertiesChanged args: {err:?}");
                        return;
                    }
                };

                let mut new_lid_closed = lid_closed;
                let mut changed = false;
                for (name, value) in args.changed_properties() {
                    trace!("changed property: {name} => {value:?}");
                    if *name != "LidClosed" {
                        continue;
                    }

                    new_lid_closed = bool::try_from(value).unwrap_or(new_lid_closed);
                    changed = true;
                }

                if !changed {
                    continue;
                }

                if new_lid_closed == lid_closed {
                    continue;
                }

                lid_closed = new_lid_closed;
                if let Err(err) = to_clarion.send(Login1ToClarion::LidClosedChanged(lid_closed)) {
                    warn!("error sending message to clarion: {err:?}");
                    return;
                };
            }
        }
    };

    // Lock requests arrive as a signal on the session object, not as a Manager
    // property. Watch the caller's own session so `loginctl lock-session` (and
    // the Shell's lock action, which uses it) starts the configured
    // `idle { lock-command }` like the idle policy does.
    let lock_future = {
        let lock_conn = async_conn.clone();
        async move {
            let session_path = match login1_session_path(&lock_conn).await {
                Ok(path) => path,
                Err(err) => {
                    warn!("error resolving the login1 session; lock requests will be ignored: {err:?}");
                    return;
                }
            };

            let session = match zbus::Proxy::new(
                &lock_conn,
                "org.freedesktop.login1",
                session_path.to_string(),
                "org.freedesktop.login1.Session",
            )
            .await
            {
                Ok(proxy) => proxy,
                Err(err) => {
                    warn!("error creating login1 Session proxy: {err:?}");
                    return;
                }
            };

            let mut lock_signals = match session.receive_signal("Lock").await {
                Ok(signals) => signals,
                Err(err) => {
                    warn!("error subscribing to login1 Lock signals: {err:?}");
                    return;
                }
            };

            while lock_signals.next().await.is_some() {
                trace!("login1 requested a session lock");
                if let Err(err) = to_clarion.send(Login1ToClarion::LockRequested) {
                    warn!("error sending the lock request to clarion: {err:?}");
                    return;
                }
            }
        }
    };

    let executor = conn.inner().executor();
    executor
        .spawn(future, "monitor login1 property changes")
        .detach();
    executor
        .spawn(lock_future, "monitor login1 lock requests")
        .detach();

    Ok(conn)
}

/// Resolves the object path of the session this process belongs to.
async fn login1_session_path(conn: &zbus::Connection) -> anyhow::Result<OwnedObjectPath> {
    let manager = zbus::Proxy::new(
        conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;

    let reply = manager.call_method("GetSession", &("auto",)).await?;
    let path: OwnedObjectPath = reply.body().deserialize()?;
    Ok(path)
}
