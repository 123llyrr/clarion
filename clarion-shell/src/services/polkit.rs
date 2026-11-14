// SPDX-License-Identifier: GPL-3.0-or-later
//! System-bus polkit agent. This thread owns registration, the helper session
//! and the queue; the UI only sees [`Event`]s and sends [`Command`]s.
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use zbus::message::Header;
use zbus::zvariant::OwnedValue;
use zbus::{interface, Connection, DBusError, Proxy};

use crate::polkit::helper::{HelperGuard, HelperSession};
use crate::polkit::{
    pick_identity, valid_cookie, AuthRequest, AuthStatus, HelperEvent, AGENT_PATH,
    AUTHORITY_IFACE, AUTHORITY_NAME, AUTHORITY_PATH,
};

use super::{Command, Event};

/// How often the service checks that polkitd is still the same owner and, if
/// registration failed, tries again. Registration itself is cheap.
const TICK: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub(super) enum AuthOutcome {
    Success,
    Cancelled,
    Failed(String),
}

#[derive(Debug, DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum PolicyKitError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Cancelled,
    Failed(String),
}

/// Messages from the agent object, the helper thread and the UI all share one
/// channel, so the service can keep a single ordered state machine.
enum Incoming {
    Command(Command),
    Begin {
        request: AuthRequest,
        reply: Sender<AuthOutcome>,
    },
    Cancel(String),
    Helper {
        cookie: String,
        event: HelperEvent,
    },
    Retry,
}

struct Agent {
    tx: Sender<Incoming>,
    connection: Connection,
}

impl Agent {
    /// polkitd runs as the `polkitd` user on current polkit releases, not as
    /// root, so a uid check would reject every real request. The authority's
    /// well-known name must still resolve to the exact connection calling us.
    async fn sender_is_authority(
        &self,
        header: &Header<'_>,
    ) -> std::result::Result<bool, zbus::fdo::Error> {
        let Some(sender) = header.sender() else {
            return Ok(false);
        };
        let dbus = zbus::fdo::DBusProxy::new(&self.connection).await?;
        let Ok(name) = AUTHORITY_NAME.try_into() else {
            return Ok(false);
        };
        match dbus.get_name_owner(name).await {
            Ok(owner) => Ok(owner.as_str() == sender.as_str()),
            Err(_) => Ok(false),
        }
    }
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    #[allow(clippy::too_many_arguments)]
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
        #[zbus(header)] header: Header<'_>,
    ) -> std::result::Result<(), PolicyKitError> {
        // The system bus policy already restricts this object to polkitd;
        // matching the authority's current owner keeps a misconfigured bus
        // (or a stale unique name) from driving the password dialog.
        let authority = self
            .sender_is_authority(&header)
            .await
            .map_err(|error| PolicyKitError::Failed(error.to_string()))?;
        if !authority {
            return Err(PolicyKitError::Failed(
                "authentication request did not come from polkitd".into(),
            ));
        }
        if !valid_cookie(&cookie) {
            return Err(PolicyKitError::Failed("invalid cookie".into()));
        }
        let Some(identity): Option<super::AuthIdentity> = pick_identity(&identities) else {
            return Err(PolicyKitError::Failed(
                "no unix-user identity in the request".into(),
            ));
        };
        let requester = crate::polkit::requester_app(&details);
        let request = AuthRequest {
            cookie,
            action_id,
            message,
            icon: (!icon_name.is_empty()).then_some(icon_name),
            app_name: requester.as_ref().and_then(|app| app.name.clone()),
            app_icon: requester.and_then(|app| app.icon),
            details: details.into_iter().collect(),
            identity,
            queued: 0,
        };
        let (reply, reply_rx) = async_channel::bounded(1);
        self.tx
            .send(Incoming::Begin { request, reply })
            .await
            .map_err(|_| PolicyKitError::Failed("agent is shutting down".into()))?;
        match reply_rx.recv().await {
            Ok(AuthOutcome::Success) => Ok(()),
            Ok(AuthOutcome::Cancelled) => Err(PolicyKitError::Cancelled),
            Ok(AuthOutcome::Failed(message)) => Err(PolicyKitError::Failed(message)),
            Err(_) => Err(PolicyKitError::Failed("agent dropped the request".into())),
        }
    }

    async fn cancel_authentication(
        &self,
        cookie: String,
        #[zbus(header)] header: Header<'_>,
    ) -> std::result::Result<(), PolicyKitError> {
        let authority = self
            .sender_is_authority(&header)
            .await
            .map_err(|error| PolicyKitError::Failed(error.to_string()))?;
        if !authority {
            return Err(PolicyKitError::Failed(
                "cancel did not come from polkitd".into(),
            ));
        }
        let _ = self.tx.send(Incoming::Cancel(cookie)).await;
        Ok(())
    }
}

struct HelperControl {
    guard: HelperGuard,
    answers: Sender<String>,
}

struct ActiveAuth {
    request: AuthRequest,
    reply: Sender<AuthOutcome>,
    helper: Option<HelperControl>,
    /// A password typed between attempts: the next helper gets it as soon as
    /// it asks its first prompt.
    pending_answer: Option<String>,
}

struct PendingAuth {
    request: AuthRequest,
    reply: Sender<AuthOutcome>,
}

struct Service {
    connection: Connection,
    events: Sender<Event>,
    tx: Sender<Incoming>,
    active: Option<ActiveAuth>,
    queue: VecDeque<PendingAuth>,
    status: Option<AuthStatus>,
    authority_owner: Option<String>,
    registered: bool,
    /// When the next registration attempt is allowed after a failure. A
    /// conflicting third-party agent is the common case, so hammering polkitd
    /// every tick would only fill the journal.
    register_retry: Option<Instant>,
    last_tick: Instant,
}

pub(super) async fn run(events: Sender<Event>, commands: Receiver<Command>) -> Result<()> {
    let connection = zbus::connection::Builder::system()?
        .method_timeout(Duration::from_secs(3))
        .build()
        .await
        .context("connect to the system bus")?;
    let (tx, rx) = async_channel::unbounded();
    let agent = Agent {
        tx: tx.clone(),
        connection: connection.clone(),
    };
    connection
        .object_server()
        .at(AGENT_PATH, agent)
        .await
        .context("serve the polkit agent object")?;
    // Commands arrive on a blocking channel from the router. A dedicated
    // thread forwards them so the D-Bus executor never blocks on the UI.
    let command_tx = tx.clone();
    std::thread::Builder::new()
        .name("clarion-polkit-commands".into())
        .spawn(move || {
            while let Ok(command) = commands.recv_blocking() {
                if command_tx.send_blocking(Incoming::Command(command)).is_err() {
                    break;
                }
            }
        })
        .context("spawn polkit command bridge")?;
    let mut service = Service {
        connection,
        events,
        tx,
        active: None,
        queue: VecDeque::new(),
        status: None,
        authority_owner: None,
        registered: false,
        register_retry: None,
        last_tick: Instant::now() - TICK,
    };
    service.run(rx).await;
    Ok(())
}

impl Service {
    async fn run(&mut self, incoming: Receiver<Incoming>) {
        loop {
            let wait = async { incoming.recv().await.ok() };
            let tick = async {
                async_io::Timer::after(TICK).await;
                Some(Incoming::Retry)
            };
            let event = futures_lite::future::or(wait, tick).await;
            let Some(event) = event else {
                break;
            };
            let shutdown = self.handle(event).await;
            if shutdown {
                break;
            }
        }
        self.fail_all("认证代理已停止", true);
    }

    async fn handle(&mut self, event: Incoming) -> bool {
        match event {
            Incoming::Retry => {
                if self.last_tick.elapsed() >= TICK {
                    self.last_tick = Instant::now();
                    self.check_authority().await;
                }
            }
            Incoming::Command(Command::Shutdown) => return true,
            Incoming::Command(Command::AuthSubmit { cookie, secret }) => {
                self.submit(&cookie, secret);
            }
            Incoming::Command(Command::AuthCancel { cookie }) => self.cancel(&cookie, true),
            Incoming::Command(_) => {}
            Incoming::Begin { request, reply } => {
                let duplicate = self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.request.cookie == request.cookie)
                    || self
                        .queue
                        .iter()
                        .any(|pending| pending.request.cookie == request.cookie);
                if duplicate {
                    let _ = reply.try_send(AuthOutcome::Failed(
                        "another request uses the same cookie".into(),
                    ));
                    return false;
                }
                self.queue.push_back(PendingAuth { request, reply });
                self.start_next();
            }
            Incoming::Cancel(cookie) => self.cancel(&cookie, true),
            Incoming::Helper { cookie, event } => self.helper_event(&cookie, event),
        }
        false
    }

    fn send_event(&self, event: Event) {
        let _ = self.events.try_send(event);
    }

    fn set_status(&mut self, status: AuthStatus) {
        if self.status.as_ref() == Some(&status) {
            return;
        }
        self.status = Some(status.clone());
        self.send_event(Event::AuthStatus(status));
    }

    async fn check_authority(&mut self) {
        let owner = match zbus::fdo::DBusProxy::new(&self.connection).await {
            Ok(dbus) => {
                let Ok(name) = AUTHORITY_NAME.try_into() else {
                    return;
                };
                dbus.get_name_owner(name)
                    .await
                    .ok()
                    .map(|owner| owner.to_string())
            }
            Err(error) => {
                self.set_status(AuthStatus::unavailable(format!(
                    "无法连接系统总线：{error}"
                )));
                return;
            }
        };
        if owner != self.authority_owner {
            let had_owner = self.authority_owner.is_some();
            if had_owner {
                // The previous polkitd owned our registration and every
                // cookie it handed out. A dialog left open would be typing
                // into a request nobody can complete.
                self.registered = false;
                self.fail_all(
                    if owner.is_none() {
                        "polkit 服务已退出"
                    } else {
                        "polkit 服务已重启"
                    },
                    true,
                );
            }
            self.authority_owner = owner.clone();
            self.register_retry = None;
            if owner.is_none() {
                self.set_status(AuthStatus::unavailable("polkit 服务未运行"));
                return;
            }
        }
        if self.authority_owner.is_some()
            && !self.registered
            && self
                .register_retry
                .is_none_or(|retry| Instant::now() >= retry)
        {
            self.register().await;
        }
    }

    async fn session_id(&self) -> Option<String> {
        if let Ok(id) = std::env::var("XDG_SESSION_ID") {
            if !id.is_empty() {
                return Some(id);
            }
        }
        let manager = Proxy::new(
            &self.connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await
        .ok()?;
        let pid = std::process::id();
        if let Ok(path) = manager
            .call::<_, _, zbus::zvariant::OwnedObjectPath>("GetSessionByPID", &(pid))
            .await
        {
            let session = Proxy::new(
                &self.connection,
                "org.freedesktop.login1",
                path.as_str(),
                "org.freedesktop.login1.Session",
            )
            .await
            .ok()?;
            let class: String = session
                .get_property("Class")
                .await
                .unwrap_or_default();
            // The user manager itself is a "manager" pseudo-session; that is
            // not where pkexec/terminal prompts come from.
            if class != "manager" {
                if let Ok(id) = session.get_property::<String>("Id").await {
                    return Some(id);
                }
            }
        }
        // A user service is tracked as that pseudo-session, so walk the user's
        // real login sessions and prefer an active graphical one.
        let user_path: zbus::zvariant::OwnedObjectPath =
            manager.call("GetUserByPID", &(pid)).await.ok()?;
        let user = Proxy::new(
            &self.connection,
            "org.freedesktop.login1",
            user_path.as_str(),
            "org.freedesktop.login1.User",
        )
        .await
        .ok()?;
        let sessions: Vec<(String, zbus::zvariant::OwnedObjectPath)> =
            user.get_property("Sessions").await.ok()?;
        let mut best: Option<(u8, String)> = None;
        for (id, path) in sessions {
            let Ok(session) = Proxy::new(
                &self.connection,
                "org.freedesktop.login1",
                path.as_str(),
                "org.freedesktop.login1.Session",
            )
            .await
            else {
                continue;
            };
            let active: bool = session.get_property("Active").await.unwrap_or(false);
            let class: String = session.get_property("Class").await.unwrap_or_default();
            let kind: String = session.get_property("Type").await.unwrap_or_default();
            let seat: (String, zbus::zvariant::OwnedObjectPath) = match session
                .get_property("Seat")
                .await
            {
                Ok(seat) => seat,
                Err(_) => continue,
            };
            if class == "manager" || !active {
                continue;
            }
            let score = if matches!(kind.as_str(), "wayland" | "x11") {
                2
            } else if !seat.0.is_empty() {
                1
            } else {
                0
            };
            if best
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
            {
                best = Some((score, id));
            }
        }
        best.map(|(_, id)| id)
    }

    async fn register(&mut self) {
        let Some(session_id) = self.session_id().await else {
            self.register_retry = Some(Instant::now() + Duration::from_secs(10));
            self.set_status(AuthStatus::unavailable(
                "无法确定当前登录会话；已跳过 polkit 代理注册",
            ));
            return;
        };
        let proxy = match Proxy::new(
            &self.connection,
            AUTHORITY_NAME,
            AUTHORITY_PATH,
            AUTHORITY_IFACE,
        )
        .await
        {
            Ok(proxy) => proxy,
            Err(error) => {
                self.register_retry = Some(Instant::now() + Duration::from_secs(10));
                self.set_status(AuthStatus::unavailable(format!(
                    "无法访问 polkit 服务：{error}"
                )));
                return;
            }
        };
        let subject = crate::polkit::unix_session_subject(&session_id);
        let locale = std::env::var("LANG").unwrap_or_else(|_| "C".to_owned());
        let options: HashMap<String, OwnedValue> =
            HashMap::from([("fallback".to_owned(), OwnedValue::from(false))]);
        let with_options: std::result::Result<(), zbus::Error> = proxy
            .call(
                "RegisterAuthenticationAgentWithOptions",
                &(subject.clone(), locale.clone(), AGENT_PATH, options),
            )
            .await;
        let result = match with_options {
            Ok(()) => Ok(()),
            Err(zbus::Error::MethodError(name, _, _))
                if matches!(
                    name.as_str(),
                    "org.freedesktop.DBus.Error.UnknownMethod"
                        | "org.freedesktop.DBus.Error.UnknownInterface"
                ) =>
            {
                let legacy: std::result::Result<(), zbus::Error> = proxy
                    .call(
                        "RegisterAuthenticationAgent",
                        &(subject, locale, AGENT_PATH),
                    )
                    .await;
                legacy
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => {
                self.registered = true;
                self.register_retry = None;
                self.set_status(AuthStatus::ready());
            }
            Err(error) => {
                self.registered = false;
                // Conflict or a transient bus error: try again later rather
                // than on every tick.
                self.register_retry = Some(Instant::now() + Duration::from_secs(10));
                self.set_status(AuthStatus::unavailable(describe_registration_error(&error)));
            }
        }
    }

    fn start_next(&mut self) {
        if self.active.is_some() {
            return;
        }
        let Some(mut pending) = self.queue.pop_front() else {
            return;
        };
        pending.request.queued = self.queue.len();
        self.send_event(Event::AuthRequest(pending.request.clone()));
        self.active = Some(ActiveAuth {
            request: pending.request,
            reply: pending.reply,
            helper: None,
            pending_answer: None,
        });
        self.start_helper();
    }

    fn start_helper(&mut self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let cookie = active.request.cookie.clone();
        let username = active.request.identity.username.clone();
        match HelperSession::open(&username, &cookie) {
            Ok(mut session) => {
                let guard = session.guard();
                let (answers, answer_rx) = async_channel::bounded::<String>(1);
                let tx = self.tx.clone();
                let thread_cookie = cookie.clone();
                let spawned = std::thread::Builder::new()
                    .name("clarion-polkit-helper".into())
                    .spawn(move || {
                        loop {
                            let event = match session.read_event() {
                                Ok(event) => event,
                                Err(error) => {
                                    let _ = tx.send_blocking(Incoming::Helper {
                                        cookie: thread_cookie.clone(),
                                        event: HelperEvent::Error(format!(
                                            "认证 helper：{error:#}"
                                        )),
                                    });
                                    HelperEvent::Closed
                                }
                            };
                            let terminal = matches!(
                                event,
                                HelperEvent::Success
                                    | HelperEvent::Failure
                                    | HelperEvent::Closed
                            );
                            let prompt = matches!(event, HelperEvent::Prompt { .. });
                            if tx
                                .send_blocking(Incoming::Helper {
                                    cookie: thread_cookie.clone(),
                                    event,
                                })
                                .is_err()
                            {
                                break;
                            }
                            if !prompt {
                                if terminal {
                                    break;
                                }
                                continue;
                            }
                            match answer_rx.recv_blocking() {
                                Ok(answer) => {
                                    if session.answer(&answer).is_err() {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    });
                if spawned.is_err() {
                    self.helper_failed(&cookie, "无法启动认证 helper 线程");
                    return;
                }
                if let Some(active) = self.active.as_mut() {
                    active.helper = Some(HelperControl { guard, answers });
                }
            }
            Err(error) => {
                let message = format!("无法启动认证 helper：{error:#}");
                self.set_status(AuthStatus::unavailable(message.clone()));
                self.helper_failed(&cookie, &message);
            }
        }
    }

    fn helper_failed(&mut self, cookie: &str, message: &str) {
        if let Some(active) = self.active.as_mut() {
            // Reap the failed conversation now: a P2P socket or a setuid child
            // must not linger while the user types a retry.
            if let Some(helper) = active.helper.take() {
                helper.guard.cancel();
            }
        }
        self.send_event(Event::AuthFailed {
            cookie: cookie.to_owned(),
            text: message.to_owned(),
        });
    }

    fn submit(&mut self, cookie: &str, secret: String) {
        let mut retry = false;
        {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if active.request.cookie != cookie {
                return;
            }
            if let Some(helper) = active.helper.as_ref() {
                // The helper may not have printed its prompt yet; the bounded
                // channel keeps the answer until it does.
                let _ = helper.answers.try_send(secret);
            } else {
                // Retry after a failed attempt: a fresh PAM session, same cookie.
                active.pending_answer = Some(secret);
                retry = true;
            }
        }
        if retry {
            self.start_helper();
        }
    }

    fn helper_event(&mut self, cookie: &str, event: HelperEvent) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.request.cookie != cookie {
            return;
        }
        match event {
            HelperEvent::Prompt { text, echo } => {
                self.send_event(Event::AuthPrompt {
                    cookie: cookie.to_owned(),
                    text,
                    echo,
                });
                // A retry answer typed before the helper was restarted belongs
                // to the password prompt. Never feed it to an echo-on prompt
                // (the login name), or the helper would authenticate nothing
                // and consume the secret in the wrong field.
                let answer = if echo {
                    None
                } else {
                    self.active
                        .as_mut()
                        .and_then(|active| active.pending_answer.take())
                };
                if let Some(answer) = answer {
                    if let Some(helper) = self.active.as_ref().and_then(|a| a.helper.as_ref()) {
                        let _ = helper.answers.try_send(answer);
                    }
                }
            }
            HelperEvent::Info(text) => self.send_event(Event::AuthInfo {
                cookie: cookie.to_owned(),
                text,
            }),
            HelperEvent::Error(text) => self.send_event(Event::AuthError {
                cookie: cookie.to_owned(),
                text,
            }),
            HelperEvent::Failure => {
                self.helper_failed(cookie, "认证失败，请重试");
            }
            HelperEvent::Closed => {
                let had_helper = self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.helper.is_some());
                if had_helper {
                    self.helper_failed(cookie, "认证未完成，请重试");
                }
            }
            HelperEvent::Success => {
                let Some(active) = self.active.take() else {
                    return;
                };
                // The helper has already sent the authority response; closing
                // the transport also reaps a setuid child that is about to exit.
                if let Some(helper) = active.helper.as_ref() {
                    helper.guard.cancel();
                }
                let _ = active.reply.try_send(AuthOutcome::Success);
                self.send_event(Event::AuthClosed {
                    cookie: cookie.to_owned(),
                });
                self.start_next();
            }
        }
    }

    fn cancel(&mut self, cookie: &str, notify: bool) {
        if let Some(active) = self.active.as_ref() {
            if active.request.cookie == cookie {
                let active = self.active.take().unwrap();
                if let Some(helper) = active.helper.as_ref() {
                    helper.guard.cancel();
                }
                let _ = active.reply.try_send(AuthOutcome::Cancelled);
                if notify {
                    self.send_event(Event::AuthClosed {
                        cookie: cookie.to_owned(),
                    });
                }
                self.start_next();
                return;
            }
        }
        if let Some(index) = self
            .queue
            .iter()
            .position(|pending| pending.request.cookie == cookie)
        {
            if let Some(pending) = self.queue.remove(index) {
                let _ = pending.reply.try_send(AuthOutcome::Cancelled);
            }
        }
    }

    fn fail_all(&mut self, reason: &str, notify: bool) {
        if let Some(active) = self.active.take() {
            if let Some(helper) = active.helper.as_ref() {
                helper.guard.cancel();
            }
            let _ = active
                .reply
                .try_send(AuthOutcome::Failed(reason.to_owned()));
            if notify {
                self.send_event(Event::AuthClosed {
                    cookie: active.request.cookie,
                });
            }
        }
        while let Some(pending) = self.queue.pop_front() {
            let _ = pending
                .reply
                .try_send(AuthOutcome::Failed(reason.to_owned()));
        }
    }
}

fn describe_registration_error(error: &zbus::Error) -> String {
    let text = error.to_string();
    if text.contains("already") || text.contains("exists") {
        return "已有其他 polkit 认证代理在运行，Clarion 无法接管".into();
    }
    format!("无法注册 polkit 认证代理：{text}")
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use futures_lite::future;

    /// Opt-in probe against a real polkitd: registers this process as the
    /// session agent, waits for the ready status, then shuts the service down.
    /// Run with `CLARION_POLKIT_LIVE_TEST=1`, never from the normal suite.
    #[test]
    #[ignore = "touches the real system bus; run with CLARION_POLKIT_LIVE_TEST=1"]
    fn registers_as_the_session_agent() {
        if std::env::var("CLARION_POLKIT_LIVE_TEST").as_deref() != Ok("1") {
            return;
        }
        let (events, event_rx) = async_channel::unbounded();
        let (commands, command_rx) = async_channel::unbounded();
        let service = std::thread::spawn(move || async_io::block_on(run(events, command_rx)));
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut registered = false;
        while Instant::now() < deadline {
            let received = async {
                event_rx.recv().await.ok()
            };
            let timeout = async {
                async_io::Timer::after(deadline.saturating_duration_since(Instant::now()))
                    .await;
                None
            };
            match async_io::block_on(future::or(received, timeout)) {
                Some(Event::AuthStatus(status)) => {
                    eprintln!("polkit live test: {status:?}");
                    if status.registered {
                        registered = true;
                        break;
                    }
                }
                Some(other) => eprintln!("polkit live test: unexpected event {other:?}"),
                None => break,
            }
        }
        let _ = commands.send_blocking(Command::Shutdown);
        drop(commands);
        let _ = service.join();
        assert!(registered, "the agent never reported a successful registration");
    }
}
